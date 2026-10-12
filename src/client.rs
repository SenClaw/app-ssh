use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use russh::ChannelMsg;
use russh::client::{Config, Handle};
use russh_keys::key::KeyPair;
use russh_sftp::client::SftpSession;
use serde::Deserialize;
use std::sync::Arc;

use async_trait::async_trait;

/// The size the browser is actually drawing the terminal at.
///
/// The remote shell wraps its own lines, so a PTY that disagrees with the
/// client redraws the prompt at the wrong column — which is what leaves
/// fragments of an earlier command sitting in the middle of the new one.
#[derive(Clone, Copy)]
pub struct TerminalSize {
    pub cols: u32,
    pub rows: u32,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self { cols: 80, rows: 24 }
    }
}

impl TerminalSize {
    /// Keep a stray or hostile value from reaching the server as a PTY size.
    pub fn sane(self) -> Self {
        Self {
            cols: self.cols.clamp(1, 1000),
            rows: self.rows.clamp(1, 1000),
        }
    }
}

/// What a binary websocket frame carries. Keystrokes travel as text frames, so
/// the two never have to be told apart by guessing at their contents.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum TerminalControl {
    Resize { cols: u32, rows: u32 },
}

#[derive(Clone)]
pub struct ClientHandler {}

#[async_trait]
impl russh::client::Handler for ClientHandler {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        _server_public_key: &russh_keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // Accept all server keys for the sake of simplicity.
        Ok(true)
    }
}

pub struct SshClient {
    pub handle: Handle<ClientHandler>,
    pub host_id: Option<String>,
}

impl SshClient {
    pub async fn connect(
        host: &str,
        port: u16,
        user: &str,
        password: Option<&str>,
        key_pair: Option<KeyPair>,
        host_id: Option<String>,
    ) -> Result<Self> {
        let config = Arc::new(Config::default());
        let sh = ClientHandler {};
        let mut handle = russh::client::connect(config, format!("{}:{}", host, port), sh).await?;

        if let Some(kp) = key_pair {
            let auth_res = handle.authenticate_publickey(user, Arc::new(kp)).await?;
            if !auth_res {
                return Err(anyhow::anyhow!("Public key authentication failed"));
            }
        } else if let Some(pwd) = password {
            let auth_res = handle.authenticate_password(user, pwd).await?;
            if !auth_res {
                return Err(anyhow::anyhow!("Password authentication failed"));
            }
        } else {
            return Err(anyhow::anyhow!("No authentication method provided"));
        }

        Ok(Self { handle, host_id })
    }

    pub async fn execute(&mut self, command: &str) -> Result<String> {
        let mut channel = self.handle.channel_open_session().await?;
        channel.exec(true, command).await?;

        let mut output = String::new();
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { ref data } => {
                    output.push_str(&String::from_utf8_lossy(data));
                }
                ChannelMsg::ExtendedData { ref data, .. } => {
                    output.push_str(&String::from_utf8_lossy(data));
                }
                ChannelMsg::Eof => break,
                ChannelMsg::ExitStatus { .. } => break,
                _ => {}
            }
        }
        Ok(output)
    }

    pub async fn get_sftp(&mut self) -> Result<SftpSession> {
        let mut channel = self.handle.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let sftp = SftpSession::new(channel.into_stream()).await?;
        Ok(sftp)
    }

    pub async fn interactive_shell(
        &mut self,
        mut socket: axum::extract::ws::WebSocket,
        size: TerminalSize,
    ) -> Result<()> {
        let size = size.sane();
        let mut channel = self.handle.channel_open_session().await?;

        // Request PTY at the size the client is showing, not a fixed 80x24:
        // the shell starts wrapping at that width the moment it draws a prompt.
        channel
            .request_pty(
                true,
                "xterm-256color",
                size.cols,
                size.rows,
                0,
                0,
                &[], // terminal modes
            )
            .await?;

        // Request shell
        channel.request_shell(true).await?;

        let (mut sender, mut receiver) = socket.split();

        let mut buf = vec![0; 4096];

        loop {
            tokio::select! {
                // There's terminal input available from the user
                msg = receiver.next() => {
                    match msg {
                        Some(Ok(axum::extract::ws::Message::Text(text))) => {
                            channel.data(text.as_bytes()).await?;
                        }
                        Some(Ok(axum::extract::ws::Message::Binary(bin))) => {
                            match serde_json::from_slice::<TerminalControl>(&bin) {
                                Ok(TerminalControl::Resize { cols, rows }) => {
                                    let size = TerminalSize { cols, rows }.sane();
                                    channel
                                        .window_change(size.cols, size.rows, 0, 0)
                                        .await?;
                                }
                                // Not a control frame: an older client sending
                                // keystrokes as binary still gets through.
                                Err(_) => channel.data(&bin[..]).await?,
                            }
                        }
                        Some(Err(_)) | None => {
                            channel.eof().await?;
                            break;
                        }
                        _ => {}
                    }
                }
                // There's an event available on the session channel
                Some(msg) = channel.wait() => {
                    match msg {
                        // Forward the bytes as they arrived. Decoding each SSH
                        // packet on its own turns any UTF-8 character that
                        // straddles two packets into replacement characters.
                        ChannelMsg::Data { ref data } => {
                            if sender
                                .send(axum::extract::ws::Message::Binary(data.to_vec()))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        ChannelMsg::ExtendedData { ref data, .. } => {
                            if sender
                                .send(axum::extract::ws::Message::Binary(data.to_vec()))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        ChannelMsg::ExitStatus { .. } | ChannelMsg::Eof => {
                            let _ = channel.eof().await;
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(())
    }
}
