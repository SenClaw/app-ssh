//! A stand-in SSH server for end-to-end tests: it accepts one user/password,
//! runs nothing, answers every exec with `ran: <command>` and appends the
//! command to a log file — so a test can prove which commands reached the
//! server and which were held back.
//!
//! cargo run --example fake_sshd -- <port> <log file>   (user `tester`, password `pw`)

use std::io::Write;
use std::sync::Arc;

use async_trait::async_trait;
use russh::server::{Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId, CryptoVec};

#[derive(Clone)]
struct Fake {
    log: Arc<std::path::PathBuf>,
}

impl russh::server::Server for Fake {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        self.clone()
    }
}

#[async_trait]
impl russh::server::Handler for Fake {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == "tester" && password == "pw" {
            Auth::Accept
        } else {
            Auth::Reject { proceed_with_methods: None }
        })
    }

    async fn channel_open_session(&mut self, _: Channel<Msg>, _: &mut Session) -> Result<bool, Self::Error> {
        Ok(true)
    }

    async fn exec_request(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).to_string();
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&*self.log)?;
        writeln!(f, "{command}")?;
        session.channel_success(channel);
        session.data(channel, CryptoVec::from(format!("ran: {command}\n")));
        session.exit_status_request(channel, 0);
        session.eof(channel);
        session.close(channel);
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port: u16 = args.get(1).and_then(|p| p.parse().ok()).unwrap_or(2222);
    let log = args.get(2).cloned().unwrap_or_else(|| "fake_sshd.log".into());
    let config = Arc::new(russh::server::Config {
        auth_rejection_time: std::time::Duration::from_millis(10),
        auth_rejection_time_initial: Some(std::time::Duration::from_millis(0)),
        keys: vec![russh_keys::key::KeyPair::generate_ed25519().unwrap()],
        ..Default::default()
    });
    let mut server = Fake { log: Arc::new(log.into()) };
    println!("fake sshd on 127.0.0.1:{port}");
    server.run_on_address(config, ("127.0.0.1", port)).await.unwrap();
}
