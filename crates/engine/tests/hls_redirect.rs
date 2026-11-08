//! A redirected playlist resolves every relative resource against its final URL.

use std::{sync::{Arc, Mutex}, time::Duration};

use engine::{CancellationToken, ClientOptions, Job, Outcome, Route};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

#[tokio::test]
async fn redirected_playlists_use_the_final_directory_for_relative_segments() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let requests = seen.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") && socket.read(&mut byte).await.unwrap_or(0) == 1 {
                request.push(byte[0]);
            }
            let path = String::from_utf8_lossy(&request).split_whitespace().nth(1).unwrap_or_default().to_owned();
            requests.lock().unwrap().push(path.clone());
            let (status, extra, body) = match path.as_str() {
                "/start.m3u8" => ("302 Found", "location: /cdn/master.m3u8\r\n", ""),
                "/cdn/master.m3u8" => ("200 OK", "", "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800\nmedia/stream.m3u8\n"),
                "/cdn/media/stream.m3u8" => ("302 Found", "location: /final/stream.m3u8\r\n", ""),
                "/final/stream.m3u8" => ("200 OK", "", "#EXTM3U\n#EXTINF:1,\nsegment.ts\n#EXT-X-ENDLIST\n"),
                "/final/segment.ts" => ("200 OK", "", "media bytes"),
                _ => ("404 Not Found", "", "wrong relative URL"),
            };
            let response = format!("HTTP/1.1 {status}\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    let dir = std::env::temp_dir().join(format!("rdm-hls-redirect-{}", std::process::id()));
    let job = Job::new(format!("http://{address}/start.m3u8").parse().unwrap(), dir.join("stream.ts"));
    let client = engine::client_with(&ClientOptions { route: Route::Direct, ..ClientOptions::default() }).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), engine::run(&client, &job, Arc::default(), CancellationToken::new())).await;
    server.abort();
    let contents = std::fs::read(&job.target).unwrap_or_default();
    let _ = std::fs::remove_dir_all(dir);
    assert!(matches!(result, Ok(Ok(Outcome::Completed))), "{result:?}: {:?}", seen.lock().unwrap());
    assert_eq!(contents, b"media bytes");
    assert!(seen.lock().unwrap().iter().any(|p| p == "/final/segment.ts"));
}
