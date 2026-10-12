use russh_sftp::client::SftpSession;
async fn run(sftp: SftpSession) {
    let dir = sftp.read_dir(".").await.unwrap();
}
