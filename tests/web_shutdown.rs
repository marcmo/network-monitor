use std::{io::ErrorKind, net::SocketAddr, time::Duration};

use network_monitor::web;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
};

async fn request(address: SocketAddr, path: &str) -> TcpStream {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut headers = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(stream.read_u8().await.unwrap());
        }
    })
    .await
    .unwrap();
    assert!(headers.starts_with(b"HTTP/1.1 200 OK\r\n"));
    stream
}

async fn assert_connection_closed(stream: &mut TcpStream) -> usize {
    let mut remaining = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut remaining))
        .await
        .expect("server retained the connection");
    assert!(
        result.is_ok() || result.is_err_and(|error| error.kind() == ErrorKind::ConnectionReset)
    );
    remaining.len()
}

#[tokio::test]
async fn shutdown_closes_a_stalled_asset_download_and_releases_the_port() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("index.html"), "Network monitor").unwrap();
    let asset_bytes = 2 * 1024 * 1024;
    std::fs::write(directory.path().join("large.wasm"), vec![0_u8; asset_bytes]).unwrap();
    let (_sender, receiver) = watch::channel(None);
    let server = web::start(
        0,
        directory.path().join("recordings.sqlite3"),
        receiver,
        directory.path().to_path_buf(),
    )
    .await
    .unwrap();
    let address = server.address;
    let mut stream = request(address, "/large.wasm").await;
    let mut live = request(address, "/api/live/stream").await;

    tokio::time::timeout(Duration::from_secs(2), server.shutdown())
        .await
        .expect("monitor shutdown waited for the stalled download")
        .unwrap();

    let _listener = TcpListener::bind(address).await.unwrap();
    assert!(assert_connection_closed(&mut stream).await < asset_bytes);
    assert_connection_closed(&mut live).await;
}

#[tokio::test]
async fn dropping_the_server_closes_owned_connections_and_releases_the_port() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("index.html"), "Network monitor").unwrap();
    let asset_bytes = 2 * 1024 * 1024;
    std::fs::write(directory.path().join("large.wasm"), vec![0_u8; asset_bytes]).unwrap();
    let (_sender, receiver) = watch::channel(None);
    let server = web::start(
        0,
        directory.path().join("recordings.sqlite3"),
        receiver,
        directory.path().to_path_buf(),
    )
    .await
    .unwrap();
    let address = server.address;
    let mut stream = request(address, "/large.wasm").await;
    let mut live = request(address, "/api/live/stream").await;

    drop(server);

    assert!(assert_connection_closed(&mut stream).await < asset_bytes);
    assert_connection_closed(&mut live).await;
    let _listener = TcpListener::bind(address).await.unwrap();
}

#[tokio::test]
async fn closing_the_snapshot_publisher_finishes_live_streams_without_stopping_the_server() {
    let directory = tempfile::tempdir().unwrap();
    let (sender, receiver) = watch::channel(None);
    let server = web::start(
        0,
        directory.path().join("recordings.sqlite3"),
        receiver,
        directory.path().to_path_buf(),
    )
    .await
    .unwrap();
    let mut live = request(server.address, "/api/live/stream").await;

    drop(sender);

    assert_connection_closed(&mut live).await;
    let mut snapshot = request(server.address, "/api/live").await;
    assert_eq!(assert_connection_closed(&mut snapshot).await, 4);
    server.shutdown().await.unwrap();
}
