//! Small HTTP responses are bounded while they arrive, including chunked bodies. Checking the
//! length after `Response::bytes()` would already have allocated an arbitrarily large body.

#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    #[error("response exceeds its size limit")]
    TooLarge,
    #[error(transparent)]
    Network(#[from] reqwest::Error),
}

pub async fn read_limited(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, BodyError> {
    if response.content_length().is_some_and(|length| length > limit as u64) {
        return Err(BodyError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > limit.saturating_sub(body.len()) {
            return Err(BodyError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    async fn response(bytes: &'static [u8]) -> (reqwest::Response, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = socket.read(&mut request).await;
            socket.write_all(bytes).await.unwrap();
            // No EOF: an oversized response must be refused before waiting for the rest.
            std::future::pending::<()>().await;
        });
        let response = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(2)).build().unwrap()
            .get(format!("http://{address}/")).send().await.unwrap();
        (response, server)
    }

    #[tokio::test]
    async fn bodies_are_bounded_before_the_server_finishes() {
        let (res, server) = response(b"HTTP/1.1 200 OK\r\nContent-Length: 5000000\r\n\r\n").await;
        assert!(matches!(read_limited(res, 4).await, Err(BodyError::TooLarge)));
        server.abort();
        let (res, server) = response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n").await;
        assert!(matches!(read_limited(res, 4).await, Err(BodyError::TooLarge)));
        server.abort();
        let (res, server) = response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n2\r\ncd\r\n0\r\n\r\n").await;
        assert_eq!(read_limited(res, 4).await.unwrap(), b"abcd");
        server.abort();
    }
}
