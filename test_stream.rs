use russh::Channel;

fn test_split(channel: russh::Channel<russh::client::Msg>) {
    let stream = channel.into_stream();
    let (mut read_half, mut write_half) = tokio::io::split(stream);
}
