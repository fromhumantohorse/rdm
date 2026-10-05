//! The YouTube module (see `ytdlp`): links YouTube refuses to the extension's own clients.

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};

use super::access::guard;
use crate::{manager::Manager, ytdlp};

pub(super) async fn state(State(manager): State<Arc<Manager>>, headers: HeaderMap) -> Result<Json<ytdlp::State>, StatusCode> {
    guard(&manager, &headers)?;
    Ok(Json(ytdlp::state()))
}

/// Starts the module's installation (a few minutes; the extension asks `/youtube/state` later).
pub(super) async fn install(State(manager): State<Arc<Manager>>, headers: HeaderMap) -> Result<Json<ytdlp::State>, StatusCode> {
    guard(&manager, &headers)?;
    manager.install_youtube_module();
    Ok(Json(ytdlp::state()))
}

#[derive(Deserialize)]
pub(super) struct Video {
    id: String,
}

#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum Extracted {
    Formats(ytdlp::Formats),
    Refused { error: String },
}

pub(super) async fn extract(State(manager): State<Arc<Manager>>, headers: HeaderMap, Json(v): Json<Video>) -> Result<Json<Extracted>, StatusCode> {
    guard(&manager, &headers)?;
    if !ytdlp::valid_id(&v.id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if ytdlp::state() != ytdlp::State::Ready {
        return Err(StatusCode::CONFLICT);
    }
    Ok(Json(match manager.youtube_links(&v.id).await {
        Ok(formats) => Extracted::Formats(formats),
        Err(e) => Extracted::Refused { error: e.to_string() },
    }))
}
