//! Browser recordings (YouTube): the page's own player feeds the data.

use std::sync::Arc;

use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use url::Url;

use super::{MAX_CHUNK, access::guard, web};
use crate::manager::{Manager, Track};

#[derive(Deserialize)]
pub(super) struct RecordStart {
    page: Url,
    filename: String,
}

#[derive(Serialize)]
pub(super) struct RecordToken {
    token: String,
}

pub(super) async fn start(
    State(manager): State<Arc<Manager>>,
    headers: HeaderMap,
    Json(req): Json<RecordStart>,
) -> Result<Json<RecordToken>, StatusCode> {
    guard(&manager, &headers)?;
    if !web(&req.page) || req.filename.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let RecordStart { page, filename } = req;
    let token = tokio::task::spawn_blocking(move || manager.start_recording(page, &filename)).await;
    token.map(|token| Json(RecordToken { token })).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Deserialize)]
pub(super) struct Chunk {
    ms: u32,
    track: Track,
}

pub(super) async fn append(
    State(manager): State<Arc<Manager>>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Query(chunk): Query<Chunk>,
    body: Body,
) -> StatusCode {
    if let Err(status) = guard(&manager, &headers) {
        return status;
    }
    // Read only now: any web page can post here, and must not get 32 MiB per request held in memory.
    let Ok(body) = to_bytes(body, MAX_CHUNK).await else { return StatusCode::PAYLOAD_TOO_LARGE };
    match manager.record_append(&token, chunk.ms, chunk.track, &body).await {
        Ok(true) => StatusCode::NO_CONTENT,
        Ok(false) => StatusCode::NOT_FOUND,
        Err(_) => StatusCode::INSUFFICIENT_STORAGE,
    }
}

#[derive(Deserialize)]
pub(super) struct Fraction {
    fraction: f64,
}

pub(super) async fn progress(
    State(manager): State<Arc<Manager>>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Json(p): Json<Fraction>,
) -> StatusCode {
    if let Err(status) = guard(&manager, &headers) {
        return status;
    }
    if !p.fraction.is_finite() {
        return StatusCode::BAD_REQUEST;
    }
    if manager.record_progress(&token, p.fraction.clamp(0.0, 1.0)) { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND }
}

pub(super) async fn finish(State(manager): State<Arc<Manager>>, headers: HeaderMap, Path(token): Path<String>) -> StatusCode {
    if let Err(status) = guard(&manager, &headers) {
        return status;
    }
    if manager.finish_recording(&token) { StatusCode::ACCEPTED } else { StatusCode::NOT_FOUND }
}

pub(super) async fn cancel(State(manager): State<Arc<Manager>>, headers: HeaderMap, Path(token): Path<String>) -> StatusCode {
    if let Err(status) = guard(&manager, &headers) {
        return status;
    }
    if manager.cancel_recording(&token, domain::Failure::RecordingCancelled) { StatusCode::NO_CONTENT } else { StatusCode::NOT_FOUND }
}
