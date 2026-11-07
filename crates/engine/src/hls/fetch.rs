//! Fetching a stream's resources: retries, byte ranges, size caps, the memory segments hold while
//! they wait to be written in order, and AES-128 decryption.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, AtomicUsize, Ordering::*},
    },
    time::Duration,
};

use aes::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7};
use futures_util::StreamExt;
use reqwest::{Client, header::HeaderMap};
use url::Url;

use super::{MAX_PART_BYTES, PlaylistError, playlist::Part};
use crate::{EngineError, RateLimit, net};

const RETRIES: u32 = 8;
/// Memory set aside for a segment before it arrives, at most (see `get_once`).
const RESERVE_MAX: u64 = 16 << 20;

pub(super) struct Http<'a> {
    pub(super) client: &'a Client,
    pub(super) headers: &'a HeaderMap,
    /// Where the playlist came from: every other URL must not escape it into the LAN, and gets
    /// its credentials only on the same site.
    pub(super) origin: &'a Url,
    pub(super) keys: Mutex<HashMap<Url, Arc<[u8]>>>,
    pub(super) limit: &'a RateLimit,
    pub(super) own_limit: &'a RateLimit,
}

/// The memory of one track's segments waiting to be written in order (see `super::MEMORY_BUDGET`).
/// Over budget, a segment waits — its connection pauses — unless it is the next one to write:
/// that one never waits, since writing it is what frees memory.
pub(super) struct Budget {
    limit: u64,
    held: AtomicU64,
    /// Position (among the segments fetched) of the next one to write.
    next: AtomicUsize,
    changed: tokio::sync::Notify,
}

impl Budget {
    pub(super) fn new(limit: u64) -> Self {
        Self { limit, held: AtomicU64::new(0), next: AtomicUsize::new(0), changed: tokio::sync::Notify::new() }
    }

    /// Segment `index` was written: the next one may go on.
    pub(super) fn written(&self, index: usize) {
        self.next.store(index + 1, Release);
        self.changed.notify_waiters();
    }
}

/// The bytes of one segment counted in a [`Budget`], given back when dropped (written, failed or
/// cancelled).
pub(super) struct Held<'a> {
    budget: &'a Budget,
    pub(super) index: usize,
    bytes: u64,
}

impl<'a> Held<'a> {
    fn new(budget: &'a Budget, index: usize) -> Self {
        Self { budget, index, bytes: 0 }
    }

    /// `n` more bytes of this segment are in memory: waits while the budget is exceeded, unless
    /// this segment is the next to write.
    async fn grow(&mut self, n: u64) {
        self.bytes += n;
        self.budget.held.fetch_add(n, AcqRel);
        loop {
            // Registered before looking, so a release in between is not missed.
            let changed = self.budget.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.budget.held.load(Acquire) <= self.budget.limit || self.budget.next.load(Acquire) == self.index {
                return;
            }
            changed.await;
        }
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        if self.bytes > 0 {
            self.budget.held.fetch_sub(self.bytes, AcqRel);
            self.budget.changed.notify_waiters();
        }
    }
}

/// One segment, decrypted, with the memory it holds in its track's budget (`index`: its position
/// among the segments fetched).
pub(super) async fn fetch_part<'b>(http: &Http<'_>, part: Part, budget: (&'b Budget, usize)) -> Result<(Vec<u8>, Option<Held<'b>>), EngineError> {
    let (data, held) = get(http, &part.url, part.range, MAX_PART_BYTES, Some(budget)).await?;
    // Dropped with the stream on cancel.
    http.limit.take(data.len()).await;
    http.own_limit.take(data.len()).await;
    let Some(key) = part.key else { return Ok((data, held)) };
    let cached = http.keys.lock().unwrap_or_else(PoisonError::into_inner).get(&key.url).cloned();
    let secret = match cached {
        Some(k) => k,
        None => {
            let k: Arc<[u8]> = get(http, &key.url, None, 1024, None).await?.0.into();
            http.keys.lock().unwrap_or_else(PoisonError::into_inner).insert(key.url.clone(), k.clone());
            k
        }
    };
    let iv = key.iv.unwrap_or_else(|| u128::from(part.seq).to_be_bytes());
    Ok((decrypt(&secret, &iv, &data)?, held))
}

/// AES-128-CBC with PKCS#7 padding, as HLS encrypts segments (`EXT-X-KEY:METHOD=AES-128`).
fn decrypt(key: &[u8], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>, EngineError> {
    cbc::Decryptor::<aes::Aes128>::new_from_slices(key, iv)
        .map_err(|_| PlaylistError::InvalidKey)?
        .decrypt_padded_vec::<Pkcs7>(data)
        .map_err(|_| PlaylistError::DecryptionFailed.into())
}

/// GET with retries, an optional (offset, length) byte range and a hard size cap. A segment's
/// bytes count in its track's `budget` (see [`Budget`]) as they arrive.
pub(super) async fn get<'b>(
    http: &Http<'_>,
    url: &Url,
    range: Option<(u64, u64)>,
    max: u64,
    budget: Option<(&'b Budget, usize)>,
) -> Result<(Vec<u8>, Option<Held<'b>>), EngineError> {
    if !net::allowed_hop(http.origin, url) {
        return Err(EngineError::LocalNetwork);
    }
    let mut attempt = 0;
    loop {
        match get_once(http, url, range, max, budget).await {
            Ok(fetched) => return Ok(fetched),
            Err(e) if attempt >= RETRIES || e.is_permanent() => return Err(e),
            Err(e) => {
                e.forget_address();
                attempt += 1;
            }
        }
        tokio::time::sleep(Duration::from_millis(250 << attempt.min(5))).await;
    }
}

async fn get_once<'b>(
    http: &Http<'_>,
    url: &Url,
    range: Option<(u64, u64)>,
    max: u64,
    budget: Option<(&'b Budget, usize)>,
) -> Result<(Vec<u8>, Option<Held<'b>>), EngineError> {
    // The page's cookies and the site's login stay on the playlist's site.
    let headers = net::headers_for(http.headers, http.origin, url).into_owned();
    let mut req = http.client.get(url.clone()).headers(headers);
    if let Some((offset, len)) = range {
        let last = offset.checked_add(len - 1).ok_or(PlaylistError::BadByteRange)?;
        req = req.header(reqwest::header::RANGE, format!("bytes={offset}-{last}"));
    }
    let res = req.send().await?.error_for_status()?;
    // A 206 for another slice (a broken proxy or CDN) would put the wrong bytes in the video.
    if let Some((offset, len)) = range
        && res.status() == reqwest::StatusCode::PARTIAL_CONTENT
        && !crate::content_range(res.headers()).is_some_and(|(start, end, _)| start == offset && end - start + 1 == len)
    {
        return Err(EngineError::RangeIgnored);
    }

    // How many bytes to read, and where the wanted slice starts within them.
    let (skip, want) = match range {
        // 206: exactly the slice.
        Some((_, len)) if res.status() == reqwest::StatusCode::PARTIAL_CONTENT => (0, len),
        // 200: the server ignored the range; read the prefix up to the slice, if that stays reasonable.
        Some((offset, len)) => (offset, offset.saturating_add(len)),
        None => (0, max),
    };
    if want > max {
        return Err(if range.is_some() { EngineError::RangeIgnored } else { PlaylistError::TooLarge.into() });
    }
    if range.is_none() && res.content_length().is_some_and(|n| n > max) {
        return Err(PlaylistError::TooLarge.into());
    }

    // Room for the announced size at once: grown by doubling, a segment would take up to twice
    // its size, and several are held at once (see `super::MAX_IN_FLIGHT`). Only up to a usual
    // segment's size: the announced length is the server's word, not data received.
    let announced = res.content_length().unwrap_or(0).min(want).min(RESERVE_MAX);
    let mut body = Vec::with_capacity(usize::try_from(announced).unwrap_or(0));
    // Dropped on failure: what this attempt read goes back to the budget.
    let mut held = budget.map(|(budget, index)| Held::new(budget, index));
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        body.extend_from_slice(&chunk);
        if let Some(held) = &mut held {
            held.grow(chunk.len() as u64).await;
        }
        let read = body.len() as u64;
        if range.is_some() && read >= want {
            break; // got the slice: the rest (if any) is not ours
        }
        if read > want {
            return Err(PlaylistError::TooLarge.into());
        }
    }
    let Some((_, len)) = range else { return Ok((body, held)) };
    let (start, end) = (usize::try_from(skip), usize::try_from(skip + len));
    match (start, end) {
        (Ok(s), Ok(e)) => body.get(s..e).map(|slice| (slice.to_vec(), held)).ok_or(EngineError::Truncated),
        _ => Err(EngineError::Truncated),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Segments arriving ahead of their turn wait once the budget is used up; the next one to write
    /// never waits (writing it is what frees memory), and once it is written the next one goes on.
    #[tokio::test]
    async fn segments_wait_for_memory_except_the_next_to_write() {
        let budget = Budget::new(100);
        let (mut head, mut later) = (Held::new(&budget, 0), Held::new(&budget, 1));
        later.grow(80).await;
        assert!(tokio::time::timeout(Duration::from_millis(50), later.grow(80)).await.is_err(), "over budget, not its turn");
        head.grow(500).await; // its turn: never held back
        assert_eq!(budget.held.load(Acquire), 660);
        let written = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            budget.written(0);
            drop(head);
        };
        let (_, went_on) = tokio::join!(written, tokio::time::timeout(Duration::from_secs(5), later.grow(10)));
        assert!(went_on.is_ok(), "now the next to write");
        drop(later);
        assert_eq!(budget.held.load(Acquire), 0, "everything given back");
    }

    /// Known answer from the `cbc` crate's documentation: AES-128-CBC, PKCS#7 padding.
    #[test]
    fn decrypts_an_aes_128_segment() {
        let (key, iv) = ([0x42u8; 16], [0x24u8; 16]);
        let segment = [
            0xc7, 0xfe, 0x24, 0x7e, 0xf9, 0x7b, 0x21, 0xf0, 0x7c, 0xbd, 0xd2, 0x6c, 0xb5, 0xd3, 0x46, 0xbf, 0xd2, 0x78, 0x67, 0xcb, 0x00, 0xd9, 0x48, 0x67,
            0x23, 0xe1, 0x59, 0x97, 0x8f, 0xb9, 0xa5, 0xf9, 0x14, 0xcf, 0xb2, 0x28, 0xa7, 0x10, 0xde, 0x41, 0x71, 0xe3, 0x96, 0xe7, 0xb6, 0xcf, 0x85, 0x9e,
        ];

        let plain = decrypt(&key, &iv, &segment).unwrap();

        assert_eq!(plain, b"hello world! this is my plaintext.");
    }

    #[test]
    fn a_wrong_key_or_a_short_key_is_an_error_not_garbage() {
        let segment = [0u8; 32];

        assert!(matches!(decrypt(&[0x42; 16], &[0; 16], &segment), Err(EngineError::Playlist(PlaylistError::DecryptionFailed))));
        assert!(matches!(decrypt(&[0x42; 8], &[0; 16], &segment), Err(EngineError::Playlist(PlaylistError::InvalidKey))));
    }
}
