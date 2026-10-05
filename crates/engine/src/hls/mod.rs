//! HLS (`.m3u8`): variant choice, alternate audio tracks, AES-128, parallel fetch with in-order append.

mod fetch;
mod playlist;

use std::{
    io::SeekFrom,
    path::Path,
    sync::{Arc, Mutex, atomic::Ordering::*},
    time::Duration,
};

use futures_util::{StreamExt, stream};
use reqwest::{Client, header::HeaderMap};
use serde::{Deserialize, Serialize};
use tokio::{
    fs::{self, OpenOptions},
    io::{AsyncSeekExt, AsyncWriteExt},
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{EngineError, Job, Outcome, Progress, RateLimit, merged::Split, state_path};
use fetch::{Budget, Http, fetch_part, get};
use playlist::{Part, Playlist, parse};
pub use playlist::Variant;

const MAX_PLAYLIST_BYTES: u64 = 16 << 20;
/// More segments than this is not a video (a day of 1-second segments is 86,400): refused rather
/// than filling memory with their addresses.
const MAX_PARTS: usize = 200_000;
/// A single segment larger than this is not a segment: refuse instead of exhausting memory.
const MAX_PART_BYTES: u64 = 256 << 20;
/// Segments fetched at once, held in memory while waiting to be written in order. 8 keep a fast
/// line busy: on a 600 MB 1080p stream, as fast as 16, with a third less memory.
const MAX_IN_FLIGHT: usize = 8;
/// Memory those segments may take together (per track): past it, a segment's connection waits for
/// the ones before it to be written. Usual segments (a few MB) never get near it; huge ones (a
/// hostile or odd stream) no longer pile up 8 × 256 MB.
const MEMORY_BUDGET: u64 = 128 << 20;
/// Resume state written this often while downloading (data synced first).
const CHECKPOINT: Duration = Duration::from_secs(20);

/// Why an HLS stream cannot be downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PlaylistError {
    #[error("not an m3u8 playlist")]
    NotAPlaylist,
    #[error("nested master playlist")]
    NestedMaster,
    #[error("empty master playlist")]
    EmptyMaster,
    #[error("separate MPEG-TS audio track is not supported")]
    SeparateTsAudio,
    #[error("empty playlist")]
    Empty,
    #[error("too many segments")]
    TooManySegments,
    #[error("bad segment url")]
    BadSegmentUrl,
    #[error("bad byte range")]
    BadByteRange,
    #[error("byte range without offset")]
    ByteRangeWithoutOffset,
    #[error("key without uri")]
    KeyWithoutUri,
    #[error("map without uri")]
    MapWithoutUri,
    #[error("DRM-protected stream")]
    Drm,
    #[error("invalid AES key")]
    InvalidKey,
    #[error("decryption failed")]
    DecryptionFailed,
    #[error("too large")]
    TooLarge,
}

pub(crate) fn is_playlist(url: &Url, content_type: Option<&str>) -> bool {
    url.path().to_ascii_lowercase().ends_with(".m3u8")
        || content_type.is_some_and(|t| t.to_ascii_lowercase().contains("mpegurl"))
}

/// What a playlist offers, without downloading it.
#[derive(Debug, Clone, Serialize)]
pub struct HlsInfo {
    /// Best first; empty for a plain media playlist.
    pub variants: Vec<Variant>,
    /// Segments are fragmented MP4 (→ `.mp4`), otherwise MPEG-TS (→ `.ts`).
    pub fmp4: bool,
}

/// Parts written so far and the file length they produced.
#[derive(Default, Serialize, Deserialize)]
struct Resume {
    done: usize,
    bytes: u64,
}

pub(crate) async fn info(client: &Client, url: &Url, headers: &HeaderMap) -> Result<HlsInfo, EngineError> {
    let limit = RateLimit::default();
    let http = Http { client, headers, origin: url, keys: Mutex::default(), limit: &limit, own_limit: &limit };
    match load(&http, url).await? {
        Playlist::Media { fmp4, .. } => Ok(HlsInfo { variants: Vec::new(), fmp4 }),
        Playlist::Master(variants) => {
            let fmp4 = match variants.first() {
                Some(best) => matches!(load(&http, &best.url).await?, Playlist::Media { fmp4: true, .. }),
                None => false,
            };
            Ok(HlsInfo { variants, fmp4 })
        }
    }
}

pub(crate) async fn run(
    client: &Client,
    job: &Job,
    progress: Arc<Progress>,
    cancel: CancellationToken,
) -> Result<Outcome, EngineError> {
    let http = Http { client, headers: &job.headers, origin: &job.url, keys: Mutex::default(), limit: &job.limit, own_limit: &job.own_limit };
    let n = job.connections;
    // Playlists load with retries: a pause must not wait for them.
    let plan = async {
        match load(&http, &job.url).await? {
            Playlist::Media { parts, .. } => Ok((parts, None)),
            Playlist::Master(variants) => choose(&http, &variants).await,
        }
    };
    let (video, audio) = tokio::select! {
        biased;
        () = cancel.cancelled() => return Ok(Outcome::Paused),
        plan = plan => plan?,
    };
    let Some(audio) = audio else {
        return download(&http, video, &job.target, n, &progress, &cancel).await;
    };
    let split = Split::new(&job.target, &cancel);
    let v = Box::pin(download(&http, video, &split.video, n, &split.progress[0], &split.stop));
    let a = Box::pin(async {
        let playlist = tokio::select! {
            biased;
            () = split.stop.cancelled() => return Ok(Outcome::Paused),
            playlist = load(&http, &audio) => playlist?,
        };
        let Playlist::Media { parts, .. } = playlist else {
            return Err(PlaylistError::NestedMaster.into());
        };
        download(&http, parts, &split.audio, (n / 4).max(1), &split.progress[1], &split.stop).await
    });
    split.finish(&progress, &job.target, v, a).await
}

/// Best variant that we can deliver with sound: separate audio only when both tracks are fMP4
/// (muxable); otherwise the best variant with muxed audio.
async fn choose(http: &Http<'_>, variants: &[Variant]) -> Result<(Vec<Part>, Option<Url>), EngineError> {
    let media = |v: &Variant| {
        let url = v.url.clone();
        async move {
            match load(http, &url).await? {
                Playlist::Media { parts, fmp4 } => Ok((parts, fmp4)),
                Playlist::Master(_) => Err(EngineError::from(PlaylistError::NestedMaster)),
            }
        }
    };
    let best = variants.first().ok_or(PlaylistError::EmptyMaster)?;
    let (parts, fmp4) = media(best).await?;
    match &best.audio {
        None => Ok((parts, None)),
        Some(audio) if fmp4 => Ok((parts, Some(audio.clone()))),
        Some(_) => match variants.iter().find(|v| v.audio.is_none()) {
            Some(muxed) => Ok((media(muxed).await?.0, None)),
            None => Err(PlaylistError::SeparateTsAudio.into()),
        },
    }
}

async fn download(
    http: &Http<'_>,
    parts: Vec<Part>,
    target: &Path,
    connections: u8,
    progress: &Progress,
    cancel: &CancellationToken,
) -> Result<Outcome, EngineError> {
    let state = state_path(target);
    let mut resume = load_state(&state).await.unwrap_or_default();
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir).await?;
    }
    let mut file = OpenOptions::new().create(true).write(true).truncate(false).open(target).await?;
    if file.metadata().await?.len() < resume.bytes || resume.done > parts.len() {
        resume = Resume::default();
    }
    file.set_len(resume.bytes).await?;
    file.seek(SeekFrom::End(0)).await?;

    let total_parts = parts.len();
    let report = |r: &Resume| {
        progress.downloaded.store(r.bytes, Relaxed);
        if let Some(total) = estimated_total(r, total_parts) {
            progress.total.store(total, Relaxed);
        }
    };
    report(&resume);
    progress.active.store(connections.into(), Relaxed);

    let budget = Budget::new(MEMORY_BUDGET);
    let mut fetched = stream::iter(parts.into_iter().skip(resume.done).enumerate())
        .map(|(index, part)| fetch_part(http, part, (&budget, index)))
        .buffered(usize::from(connections).clamp(1, MAX_IN_FLIGHT));

    // A crash or a kill mid-download loses at most this much (the segmented engine does the same).
    let mut checkpoint = tokio::time::interval_at(tokio::time::Instant::now() + CHECKPOINT, CHECKPOINT);
    let result = loop {
        let next = tokio::select! {
            biased;
            () = cancel.cancelled() => break Ok(Outcome::Paused),
            _ = checkpoint.tick() => {
                if file.flush().await.is_ok() && file.sync_data().await.is_ok() {
                    let _ = save_state(&state, &resume).await;
                }
                continue;
            }
            next = fetched.next() => next,
        };
        match next {
            None => break Ok(Outcome::Completed),
            Some(Err(e)) => break Err(e),
            Some(Ok((data, held))) => {
                if let Err(e) = file.write_all(&data).await {
                    break Err(e.into());
                }
                // Its memory goes back (as `held` drops), and the next segment may grow.
                if let Some(held) = held {
                    budget.written(held.index);
                }
                resume.done += 1;
                resume.bytes += data.len() as u64;
                report(&resume);
            }
        }
    };
    progress.active.store(0, Relaxed);
    file.flush().await?;

    match result {
        Ok(Outcome::Completed) if resume.bytes == 0 => Err(EngineError::Empty),
        Ok(Outcome::Completed) => {
            let _ = fs::remove_file(&state).await;
            progress.total.store(resume.bytes, Relaxed);
            Ok(Outcome::Completed)
        }
        other => {
            // Data on disk first, then an atomic state (temp + rename), like the segmented engine.
            file.sync_data().await?;
            save_state(&state, &resume).await?;
            other
        }
    }
}

/// The stream's size, judged from the segments written so far (`None` before the first one).
fn estimated_total(written: &Resume, parts: usize) -> Option<u64> {
    let (done, parts) = (u128::try_from(written.done).ok()?, u128::try_from(parts).ok()?);
    let total = u128::from(written.bytes).checked_mul(parts)?.checked_div(done)?;
    Some(u64::try_from(total).unwrap_or(u64::MAX))
}

async fn save_state(state: &Path, resume: &Resume) -> std::io::Result<()> {
    crate::write_atomic(state, &serde_json::to_vec(resume)?).await
}

async fn load(http: &Http<'_>, url: &Url) -> Result<Playlist, EngineError> {
    let (body, _) = get(http, url, None, MAX_PLAYLIST_BYTES, None).await?;
    parse(url, &String::from_utf8_lossy(&body))
}

async fn load_state(path: &Path) -> Option<Resume> {
    serde_json::from_slice(&fs::read(path).await.ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_is_judged_from_the_segments_written() {
        let after = |done, bytes| Resume { done, bytes };

        assert_eq!(estimated_total(&after(0, 0), 10), None, "nothing written yet");
        assert_eq!(estimated_total(&after(2, 2_000), 10), Some(10_000));
        assert_eq!(estimated_total(&after(1, u64::MAX / 2), MAX_PARTS), Some(u64::MAX), "huge segments: capped, no overflow");
    }
}
