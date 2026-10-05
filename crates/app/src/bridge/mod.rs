//! Loopback HTTP bridge the browser extension talks to.
//!
//! Threat model: any web page, any other browser extension, and any other account of the computer
//! can send requests to 127.0.0.1. Defences:
//! - the account: a connection from a program of another account is closed before it is read
//!   (see `local::peer_is_mine`) — a program, unlike a browser, can forge any `Origin`;
//! - `Origin`: browsers always set it on cross-origin requests; only *our* extension ID passes
//!   (pinned by the manifest `key`); Firefox extensions have a random per-install origin: paired
//!   by the native connector (`POST /pair`), or else accepted once the user approved it (Firefox
//!   sends `Origin` on POST only: the extension asks with `POST /ping` when it starts);
//! - no `Origin` (a local program — the native connector, `rdm --quit`, a second launch): only
//!   with the token RDM writes in the user's private files (see `local`); without it, only
//!   `GET /ping` and `GET /config` naming their browser (`x-rdm-browser`: the Firefox extension's
//!   GETs carry no `Origin`; a web page cannot add that header, so it cannot tell RDM is here);
//! - `Host`: must be the loopback address, which defeats DNS rebinding;
//! - JSON bodies (force a CORS preflight we never answer), 64 KiB cap, http(s) URLs only.

mod access;
mod recording;
mod routes;
mod running;
mod youtube;

use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};
use tokio::net::TcpListener;
use url::Url;

use crate::{local, manager::Manager, settings::BRIDGE_PORT};
pub use running::{forward, quit_running};

/// ID of the RDM extension, derived from the public `key` in `extension/manifest.json`.
pub const EXTENSION_ORIGIN: &str = "chrome-extension://cgailhenfaoohkakpdacohcnmppepjjl";
/// The same extension installed from the Chrome Web Store, which signs it with its own key.
pub const STORE_EXTENSION_ORIGIN: &str = "chrome-extension://adoaoaomelojpakojcmkaclkhopkeljp";
const MAX_BODY: usize = 64 * 1024;
const MAX_CHUNK: usize = 32 << 20;

pub async fn bind() -> std::io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, BRIDGE_PORT)).await
}

pub async fn serve(manager: Arc<Manager>, listener: TcpListener) {
    let app = Router::new()
        // POST: Firefox sends `Origin` on POST only, so this is how its extension pairs.
        .route("/ping", get(routes::ping_read).post(routes::ping))
        .route("/config", get(routes::config))
        .route("/uninstalled", post(routes::uninstalled))
        .route("/installed", post(routes::installed))
        .route("/add", post(routes::add))
        .route("/probe", post(routes::probe))
        .route("/check", post(routes::check))
        .route("/show", post(routes::show))
        .route("/quit", post(routes::quit))
        .route("/pair", post(routes::pair))
        .route("/youtube/state", post(youtube::state))
        .route("/youtube/install", post(youtube::install))
        .route("/youtube/extract", post(youtube::extract))
        .route("/record/start", post(recording::start))
        .route("/record/{token}/progress", post(recording::progress))
        .route("/record/{token}/finish", post(recording::finish))
        .route("/record/{token}/cancel", post(recording::cancel))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        // Media chunks from the browser's player: the only route with large bodies.
        .route("/record/{token}/append", post(recording::append).layer(DefaultBodyLimit::max(MAX_CHUNK)))
        .with_state(manager);
    let _ = axum::serve(OwnAccount(listener), app).await;
}

/// The bridge's listener, letting in only the connections of this account's programs (browsers,
/// the native connector, `rdm --quit`); the others are closed at once.
struct OwnAccount(TcpListener);

impl axum::serve::Listener for OwnAccount {
    type Io = tokio::net::TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let port = self.0.local_addr().map_or(BRIDGE_PORT, |a| a.port());
        loop {
            let (io, peer) = axum::serve::Listener::accept(&mut self.0).await;
            if local::peer_is_mine(peer, port) {
                return (io, peer);
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

fn web(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The account check lets this process's own requests through, end to end over HTTP.
    #[tokio::test]
    async fn the_users_own_programs_get_through() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route("/ping", get(|| async { "rdm" }));
        tokio::spawn(async move { axum::serve(OwnAccount(listener), app).await });
        let client = running::local_client().unwrap();
        let answer = client.get(format!("http://127.0.0.1:{port}/ping")).send().await.unwrap().text().await.unwrap();
        assert_eq!(answer, "rdm");
    }
}
