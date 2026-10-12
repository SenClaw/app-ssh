use russh_sftp::client::SftpSession;

pub async fn test_sftp(sftp: &SftpSession) {
    let _dir = sftp.read_dir(".").await.unwrap();
    let _file: Vec<u8> = sftp.read("test.txt").await.unwrap();
}
