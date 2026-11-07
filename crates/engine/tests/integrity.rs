//! Servers whose metadata changes or omits the file length must never yield a corrupt success.

use std::{path::PathBuf, sync::Arc, time::Duration};

use engine::{CancellationToken, ClientOptions, EngineError, Job, Outcome, Progress, Route};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn serve(responses: Vec<Vec<u8>>) -> url::Url {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/file.bin", listener.local_addr().unwrap()).parse().unwrap();
    tokio::spawn(async move {
        for response in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0; 1];
            while !request.ends_with(b"\r\n\r\n") {
                if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                    return;
                }
                request.push(byte[0]);
            }
            let _ = socket.write_all(&response).await;
        }
    });
    url
}

fn target(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rdm-integrity-{}-{name}", std::process::id())).join("file.bin")
}

fn client() -> engine::Client {
    engine::client_with(&ClientOptions { route: Route::Direct, ..ClientOptions::default() }).unwrap()
}

#[tokio::test]
async fn an_unknown_length_becomes_the_received_length_on_completion() {
    let url = serve(vec![b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n5\r\nhello\r\n0\r\n\r\n".to_vec()]).await;
    let path = target("unknown-length");
    let job = Job::new(url, path.clone());
    let progress = Arc::new(Progress::default());
    let result = engine::run(&client(), &job, progress.clone(), CancellationToken::new()).await;
    assert_eq!(result.unwrap(), Outcome::Completed);
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    assert_eq!(progress.snapshot(), (5, 5, 0));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[tokio::test]
async fn another_version_of_the_same_size_cannot_continue_a_download() {
    let url = serve(vec![
        b"HTTP/1.1 206 Partial Content\r\ncontent-range: bytes 0-15/16\r\ncontent-length: 16\r\netag: \"v1\"\r\nconnection: close\r\n\r\noldbytes".to_vec(),
        b"HTTP/1.1 206 Partial Content\r\ncontent-range: bytes 8-15/16\r\ncontent-length: 8\r\netag: \"v2\"\r\nconnection: close\r\n\r\nnewbytes".to_vec(),
    ])
    .await;
    let path = target("version-changed");
    let mut job = Job::new(url, path.clone());
    job.connections = 1;
    let result = tokio::time::timeout(Duration::from_secs(10), engine::run(&client(), &job, Arc::default(), CancellationToken::new())).await.unwrap();
    assert!(matches!(result, Err(EngineError::FileChanged)), "{result:?}");
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..8], b"oldbytes");
    assert_eq!(&bytes[8..], &[0; 8], "the new version was never mixed into the file");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// Regression: every attempt asked for a range again, and the download ended in failure, where a
/// plain request gives the file.
#[tokio::test]
async fn a_partial_answer_of_unknown_total_falls_back_to_a_plain_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/file.bin", listener.local_addr().unwrap()).parse().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0; 1];
            while !request.ends_with(b"\r\n\r\n") && socket.read(&mut byte).await.unwrap_or(0) == 1 {
                request.push(byte[0]);
            }
            let ranged = String::from_utf8_lossy(&request).to_ascii_lowercase().contains("range:");
            let answer: &[u8] = if ranged {
                b"HTTP/1.1 206 Partial Content\r\ncontent-range: bytes 0-4/*\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello"
            } else {
                b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello"
            };
            let _ = socket.write_all(answer).await;
        }
    });
    let path = target("unknown-total");
    let job = Job::new(url, path.clone());
    let result = tokio::time::timeout(Duration::from_secs(10), engine::run(&client(), &job, Arc::default(), CancellationToken::new())).await.unwrap();
    assert_eq!(result.unwrap(), Outcome::Completed);
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[tokio::test]
async fn a_probe_rejects_a_partial_answer_that_starts_at_the_wrong_byte() {
    let url = serve(vec![b"HTTP/1.1 206 Partial Content\r\ncontent-range: bytes 1-1/10\r\ncontent-length: 1\r\nconnection: close\r\n\r\nx".to_vec()]).await;
    assert!(matches!(engine::probe_once(&client(), &url, &engine::HeaderMap::new()).await, Err(EngineError::RangeIgnored)));
}
