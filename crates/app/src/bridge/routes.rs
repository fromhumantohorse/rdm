//! The routes of the extension and of local programs: is RDM here, what it captures, a download to
//! take, a stream's qualities, a link's check; pairing, showing the window, quitting.

use std::{sync::Arc, time::Duration};

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header::ORIGIN},
};
use serde::{Deserialize, Serialize};

use super::{
    access::{Access, browser_of, guard, guard_access},
    web,
};
use crate::manager::{self, AddRequest, Manager};

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) async fn ping(State(manager): State<Arc<Manager>>, headers: HeaderMap) -> Result<&'static str, StatusCode> {
    guard(&manager, &headers)?;
    Ok(concat!("rdm ", env!("CARGO_PKG_VERSION")))
}

/// `GET /ping`: is RDM here (the Firefox extension's `GET`s carry no `Origin`).
pub(super) async fn ping_read(State(manager): State<Arc<Manager>>, headers: HeaderMap) -> Result<&'static str, StatusCode> {
    guard_access(&manager, &headers, Access::Read)?;
    Ok(concat!("rdm ", env!("CARGO_PKG_VERSION")))
}

#[derive(Serialize)]
pub(super) struct Config {
    /// Extensions the browser should hand over (space-separated), from the user's settings.
    captured: String,
    /// The user removed the extension from RDM's window: it uninstalls itself.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    uninstall: bool,
}

pub(super) async fn config(State(manager): State<Arc<Manager>>, headers: HeaderMap) -> Result<Json<Config>, StatusCode> {
    guard_access(&manager, &headers, Access::Read)?;
    let uninstall = browser_of(&headers).is_some_and(|b| manager.uninstall_requested(b));
    Ok(Json(Config { captured: manager.settings().captured, uninstall }))
}

/// The extension got the removal request and uninstalls itself now.
pub(super) async fn uninstalled(State(manager): State<Arc<Manager>>, headers: HeaderMap) -> StatusCode {
    if let Err(status) = guard(&manager, &headers) {
        return status;
    }
    let Some(browser) = browser_of(&headers).map(str::to_owned) else { return StatusCode::BAD_REQUEST };
    match tokio::task::spawn_blocking(move || manager.extension_uninstalled(&browser)).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[derive(Serialize)]
pub(super) struct Installed {
    /// RDM opened the browser to install the extension: the tab it opened can be closed.
    close_tab: bool,
}

/// The extension has just been installed (or updated).
pub(super) async fn installed(State(manager): State<Arc<Manager>>, headers: HeaderMap) -> Result<Json<Installed>, StatusCode> {
    guard(&manager, &headers)?;
    let browser = browser_of(&headers).unwrap_or_default();
    Ok(Json(Installed { close_tab: manager.installed_by_rdm(browser) }))
}

pub(super) async fn add(State(manager): State<Arc<Manager>>, headers: HeaderMap, Json(req): Json<AddRequest>) -> StatusCode {
    if let Err(status) = guard(&manager, &headers) {
        return status;
    }
    if !web(&req.url) || req.audio_url.as_ref().is_some_and(|u| !web(u)) {
        return StatusCode::BAD_REQUEST;
    }
    match tokio::task::spawn_blocking(move || manager.add_from_browser(req)).await {
        Ok(true) => StatusCode::ACCEPTED,
        Ok(false) => StatusCode::TOO_MANY_REQUESTS,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Qualities of an HLS stream, for the extension's quality menu (fetched with the page's cookies).
pub(super) async fn probe(
    State(manager): State<Arc<Manager>>,
    headers: HeaderMap,
    Json(req): Json<AddRequest>,
) -> Result<Json<engine::HlsInfo>, StatusCode> {
    guard(&manager, &headers)?;
    if !web(&req.url) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let request_headers = manager::header_map(&req);
    // An unusable proxy: refused rather than sent around it.
    let client = manager.client(&req.url).await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    let info = engine::hls_info(&client, &req.url, &request_headers);
    match tokio::time::timeout(PROBE_TIMEOUT, info).await {
        Ok(Ok(info)) => Ok(Json(info)),
        Ok(Err(_)) => Err(StatusCode::BAD_GATEWAY),
        Err(_) => Err(StatusCode::GATEWAY_TIMEOUT),
    }
}

#[derive(Serialize)]
pub(super) struct Check {
    ok: bool,
    /// HTTP status when the server refused (e.g. 403: link bound to another client or token).
    status: Option<u16>,
    size: Option<u64>,
}

/// Does this media link really download from here, with these headers? Lets the extension offer
/// only working YouTube formats instead of links that would fail with 403 later.
pub(super) async fn check(
    State(manager): State<Arc<Manager>>,
    headers: HeaderMap,
    Json(req): Json<AddRequest>,
) -> Result<Json<Check>, StatusCode> {
    guard(&manager, &headers)?;
    if !web(&req.url) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut request_headers = manager::header_map(&req);
    let client = manager.client(&req.url).await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    let verdict = async {
        let p = engine::probe(&client, &req.url, &request_headers).await?;
        // The server took another identity than the one asked for: so does the last-byte check.
        if let Some(agent) = &p.agent {
            request_headers.insert(engine::header::USER_AGENT, agent.clone());
        }
        // Some servers (YouTube without its anti-bot token) serve the beginning and refuse the rest:
        // the last byte must be reachable too.
        if let Some(size) = p.size.filter(|&s| s > 1 && p.ranges) {
            let last = client
                .get(req.url.clone())
                .headers(request_headers.clone())
                .header(axum::http::header::RANGE.as_str(), format!("bytes={}-{}", size - 1, size - 1))
                .send()
                .await?;
            let status = last.status().as_u16();
            if status != 206 {
                return Ok(Check { ok: false, status: Some(status), size: p.size });
            }
        }
        Ok::<_, engine::EngineError>(Check { ok: p.size != Some(0), status: None, size: p.size })
    };
    let check = match tokio::time::timeout(CHECK_TIMEOUT, verdict).await {
        Ok(Ok(check)) => check,
        Ok(Err(engine::EngineError::Http(e))) => Check { ok: false, status: e.status().map(|s| s.as_u16()), size: None },
        _ => Check { ok: false, status: None, size: None },
    };
    Ok(Json(check))
}

#[derive(Deserialize)]
pub(super) struct Pair {
    origin: String,
}

/// The native connector pairs its Firefox extension (see `native`). Local programs only, like
/// `/quit`: a browser always sends `Origin` with a POST.
pub(super) async fn pair(State(manager): State<Arc<Manager>>, headers: HeaderMap, Json(p): Json<Pair>) -> StatusCode {
    if headers.contains_key(ORIGIN) {
        return StatusCode::FORBIDDEN;
    }
    if let Err(status) = guard(&manager, &headers) {
        return status;
    }
    match tokio::task::spawn_blocking(move || manager.pair_firefox(&p.origin)).await {
        Ok(true) => StatusCode::NO_CONTENT,
        Ok(false) => StatusCode::BAD_REQUEST,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// `rdm --quit` (the installer, before replacing files): quit as from the tray's "Quitter". Local
/// programs only: browsers always send `Origin` with a POST, so neither a web page nor an
/// extension can close RDM.
pub(super) async fn quit(State(manager): State<Arc<Manager>>, headers: HeaderMap) -> StatusCode {
    if headers.contains_key(ORIGIN) {
        return StatusCode::FORBIDDEN;
    }
    if let Err(status) = guard(&manager, &headers) {
        return status;
    }
    manager.request_quit();
    StatusCode::ACCEPTED
}

pub(super) async fn show(State(manager): State<Arc<Manager>>, headers: HeaderMap) -> StatusCode {
    // The Firefox extension's toolbar button (not approved yet: before the guard refuses it): an
    // explicit request, so a put-off approval question returns. Not for a web page's request.
    if headers.get(ORIGIN).is_some_and(|o| o.as_bytes().starts_with(b"moz-extension://")) {
        manager.firefox_wake();
    }
    if let Err(status) = guard(&manager, &headers) {
        return status;
    }
    manager.show();
    StatusCode::NO_CONTENT
}
