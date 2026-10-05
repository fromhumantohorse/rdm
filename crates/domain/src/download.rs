use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};
use url::Url;
use uuid::Uuid;

use crate::{Checksum, Failure};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DownloadId(Uuid);

impl DownloadId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for DownloadId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Queued,
    Running,
    Paused,
    Completed,
    Failed(#[serde(deserialize_with = "Failure::deserialize_saved")] Failure),
}

/// How the bytes are obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Source {
    /// Fetched by the engine from `url` (resumable).
    #[default]
    Http,
    /// Streamed in by the browser while its player plays `url` (cannot be resumed by RDM).
    Recording,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DownloadError {
    #[error("invalid transition from {from:?} to {to:?}")]
    InvalidTransition { from: Status, to: Status },
}

/// Aggregate root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: DownloadId,
    pub url: Url,
    pub target: PathBuf,
    /// At most this many connections: 1 to `MAX_CONNECTIONS`, whatever a saved list says.
    #[serde(deserialize_with = "connections_in_range")]
    connections: u8,
    /// Audio stream muxed into `target` together with the video-only `url`.
    #[serde(default)]
    pub audio: Option<Url>,
    #[serde(default)]
    pub source: Source,
    /// Named queue it waits in (0 = the main queue).
    #[serde(default)]
    pub queue: u32,
    /// Its own speed cap in KiB/s, on top of the global one (0 = none).
    #[serde(default)]
    pub speed_limit_kib: u32,
    /// Expected checksum, verified once complete.
    #[serde(default, deserialize_with = "Checksum::deserialize_saved")]
    pub checksum: Option<Checksum>,
    /// Accept an invalid TLS certificate for this download only (the user's explicit choice).
    #[serde(default)]
    pub insecure: bool,
    status: Status,
}

impl Download {
    pub fn new(url: Url, target: PathBuf, connections: u8) -> Self {
        Self {
            id: DownloadId::new(),
            url,
            target,
            connections: connections.clamp(1, crate::MAX_CONNECTIONS),
            audio: None,
            source: Source::Http,
            queue: 0,
            speed_limit_kib: 0,
            checksum: None,
            insecure: false,
            status: Status::Queued,
        }
    }

    /// A recording in progress: running from the start, fed by the browser.
    pub fn recording(page: Url, target: PathBuf) -> Self {
        Self { source: Source::Recording, status: Status::Running, ..Self::new(page, target, 1) }
    }

    pub fn is_recording(&self) -> bool {
        self.source == Source::Recording
    }

    pub const fn connections(&self) -> u8 {
        self.connections
    }

    pub const fn status(&self) -> &Status {
        &self.status
    }

    pub fn start(&mut self) -> Result<(), DownloadError> {
        self.transition(matches!(self.status, Status::Queued | Status::Paused | Status::Failed(_)), Status::Running)
    }

    /// Back into the queue: the scheduler starts it when a slot frees up. Recordings cannot be
    /// re-queued: only the browser can feed them.
    pub fn enqueue(&mut self) -> Result<(), DownloadError> {
        let allowed = !self.is_recording() && matches!(self.status, Status::Paused | Status::Failed(_));
        self.transition(allowed, Status::Queued)
    }

    /// Running → back in the queue after a transient failure (network down, busy server): the
    /// scheduler retries it later. Recordings cannot: only the browser can feed them.
    pub fn retry_later(&mut self) -> Result<(), DownloadError> {
        let allowed = !self.is_recording() && self.status == Status::Running;
        self.transition(allowed, Status::Queued)
    }

    pub fn category(&self) -> crate::Category {
        crate::Category::of(&self.target.to_string_lossy())
    }

    pub fn pause(&mut self) -> Result<(), DownloadError> {
        self.transition(matches!(self.status, Status::Running | Status::Queued), Status::Paused)
    }

    pub fn complete(&mut self) -> Result<(), DownloadError> {
        self.transition(self.status == Status::Running, Status::Completed)
    }

    pub fn fail(&mut self, failure: Failure) -> Result<(), DownloadError> {
        self.transition(self.status == Status::Running, Status::Failed(failure))
    }

    fn transition(&mut self, allowed: bool, next: Status) -> Result<(), DownloadError> {
        if !allowed {
            return Err(DownloadError::InvalidTransition { from: self.status.clone(), to: next });
        }
        self.status = next;
        Ok(())
    }
}

fn connections_in_range<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    Ok(u8::deserialize(deserializer)?.clamp(1, crate::MAX_CONNECTIONS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Download {
        Download::new("https://x.io/a.zip".parse().unwrap(), "a.zip".into(), 32)
    }

    #[test]
    fn a_download_runs_pauses_resumes_then_completes_for_good() {
        let mut d = sample();
        d.start().unwrap();
        d.pause().unwrap();
        d.start().unwrap();
        d.complete().unwrap();
        assert!(d.start().is_err());
        assert!(d.enqueue().is_err());
    }

    #[test]
    fn recordings_run_at_once_and_cannot_be_requeued() {
        let mut r = Download::recording("https://www.youtube.com/watch?v=x".parse().unwrap(), "v.mp4".into());
        assert!(r.is_recording());
        assert_eq!(r.status(), &Status::Running);
        r.fail(Failure::RecordingAbandoned).unwrap();
        assert!(r.enqueue().is_err());
    }

    #[test]
    fn a_refused_transition_says_from_where_to_where() {
        let mut d = sample();

        let refused = d.complete();

        assert_eq!(refused, Err(DownloadError::InvalidTransition { from: Status::Queued, to: Status::Completed }));
    }

    #[test]
    fn a_failure_is_saved_as_a_fact_and_read_back() {
        let mut d = sample();
        d.start().unwrap();
        d.fail(Failure::Status(404)).unwrap();

        let saved = serde_json::to_string(&d).unwrap();
        let loaded: Download = serde_json::from_str(&saved).unwrap();

        assert_eq!(loaded.status(), &Status::Failed(Failure::Status(404)));
    }

    /// RDM 0.3.11 and older saved the reason as a sentence, in the language of the time.
    #[test]
    fn a_failure_saved_as_a_sentence_still_loads() {
        let json = r#"{"id":"67e55044-10b1-426f-9247-bb680e5fe0c8","url":"https://x.io/a.zip","target":"a.zip",
            "connections":8,"status":{"Failed":"fichier introuvable (404)"}}"#;

        let d: Download = serde_json::from_str(json).unwrap();

        assert_eq!(d.status(), &Status::Failed(Failure::Text("fichier introuvable (404)".into())));
    }

    #[test]
    fn older_saved_downloads_still_load() {
        let json = r#"{"id":"67e55044-10b1-426f-9247-bb680e5fe0c8","url":"https://x.io/a.zip","target":"a.zip","size":null,"connections":8,"status":"Paused"}"#;
        let d: Download = serde_json::from_str(json).unwrap();
        assert_eq!(d.status(), &Status::Paused);
        assert!(d.audio.is_none());
    }

    #[test]
    fn a_saved_connection_count_is_brought_back_within_bounds() {
        let saved = |connections: u8| {
            let json = format!(r#"{{"id":"67e55044-10b1-426f-9247-bb680e5fe0c8","url":"https://x.io/a.zip","target":"a.zip","connections":{connections},"status":"Paused"}}"#);
            serde_json::from_str::<Download>(&json).unwrap().connections()
        };

        assert_eq!([saved(0), saved(8), saved(255)], [1, 8, crate::MAX_CONNECTIONS]);
    }

    #[test]
    fn an_unreadable_saved_checksum_is_dropped_not_the_download() {
        let json = r#"{"id":"67e55044-10b1-426f-9247-bb680e5fe0c8","url":"https://x.io/a.zip","target":"a.zip",
            "connections":8,"checksum":"sha256:abc","status":"Paused"}"#;

        let d: Download = serde_json::from_str(json).unwrap();

        assert_eq!(d.checksum, None);
    }

    #[test]
    fn transient_failures_go_back_to_the_queue() {
        let mut d = sample();
        assert!(d.retry_later().is_err(), "only a running download");
        d.start().unwrap();
        d.retry_later().unwrap();
        assert_eq!(d.status(), &Status::Queued);
        let mut r = Download::recording("https://www.youtube.com/watch?v=x".parse().unwrap(), "v.mp4".into());
        assert!(r.retry_later().is_err());
    }

    #[test]
    fn failed_can_be_requeued() {
        let mut d = sample();
        d.start().unwrap();
        d.fail(Failure::NoData).unwrap();
        d.enqueue().unwrap();
        assert_eq!(d.status(), &Status::Queued);
    }
}
