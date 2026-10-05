//! The RDM already running, seen from a second launch or from `rdm --quit`: hand it a link, bring
//! it forward, ask it to quit.

use std::time::Duration;

use url::Url;

use super::bind;
use crate::{local, settings::BRIDGE_PORT};

/// For talking to the running RDM: never through a proxy (a system proxy would get 127.0.0.1).
pub(super) fn local_client() -> Option<engine::Client> {
    engine::client_with(&engine::ClientOptions { route: engine::Route::Direct, ..engine::ClientOptions::default() }).ok()
}

/// A request to the running RDM, as a local program (see `local`): only when the program on the
/// bridge's port runs as this user, and with the token that proves this program does too.
fn local_post(path: &str) -> Option<reqwest::RequestBuilder> {
    if !local::bridge_is_ours() {
        return None;
    }
    let request = local_client()?.post(format!("http://127.0.0.1:{BRIDGE_PORT}{path}"));
    Some(match local::read_token() {
        Some(token) => request.header(local::TOKEN_HEADER, token),
        None => request, // an older RDM: it asks for none
    })
}

/// `rdm --quit`: asks a running RDM to quit and waits (bounded) until it is gone. Starts nothing.
pub async fn quit_running() {
    let Some(request) = local_post("/quit") else { return };
    let asked = request
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success());
    if !asked {
        return;
    }
    // The port is let go when the process ends (downloads saved first).
    for _ in 0..80 {
        if bind().await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Second launch: hands the URL (or just "show yourself") to the running instance; `true` when it
/// took the request.
pub async fn forward(url: Option<&Url>) -> bool {
    let request = match url {
        Some(url) => local_post("/add").map(|r| r.header("content-type", "application/json").body(serde_json::json!({ "url": url }).to_string())),
        None => local_post("/show"),
    };
    let Some(request) = request else { return false };
    request.timeout(Duration::from_secs(5)).send().await.is_ok_and(|r| r.status().is_success())
}
