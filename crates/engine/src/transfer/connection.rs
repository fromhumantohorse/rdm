//! One connection of a download: it fetches its piece, then takes over part of another, rides out
//! failures (or hands its piece back to the others), and writes what it receives at its offset.

use std::{
    cmp::min,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::*},
    },
    time::Duration,
};

use futures_util::StreamExt;
use reqwest::{Response, StatusCode, header::RANGE};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{Ctx, MIN_BUF, disk::Disk};
use crate::{EngineError, slots::Slot};

/// Data kept in a buffer is written after this long at most (slow connections).
const FLUSH_AFTER: Duration = Duration::from_secs(5);
/// Longest wait between two attempts of the last connection (network down, busy server).
const MAX_BACKOFF: Duration = Duration::from_secs(15);
/// A server without range support restarts the file from zero after a cut: at most this often.
const MAX_RESTARTS: u32 = 5;

pub(super) async fn worker(ctx: Arc<Ctx>, mut slot: Arc<Slot>, mut active: Active, mut first: Option<Response>) -> Result<(), EngineError> {
    // This worker's last request was accepted: its next one follows it (see `Pace::refuse`).
    let mut followed = false;
    loop {
        if !fetch_with_retry(&ctx, &mut slot, &mut active, followed, &mut first).await? {
            return Ok(()); // this connection closed: its piece went back to the others
        }
        followed = true;
        if ctx.stop.is_cancelled() || !ctx.ranges {
            return Ok(());
        }
        // Connection trouble lowered the limit: this one closes instead of taking more work.
        if ctx.progress.active.load(Acquire) > ctx.pace.limit() && active.try_leave() {
            return Ok(());
        }
        match ctx.slots.steal(ctx.pace.spare_requests(), !ctx.pace.counts_requests()) {
            Some(next) => slot = next,
            None => return Ok(()),
        }
    }
}

/// `Ok(true)`: the piece is done (or the download stopped). `Ok(false)`: this connection failed
/// while others keep going; the piece was handed back to them.
async fn fetch_with_retry(ctx: &Ctx, slot: &mut Arc<Slot>, active: &mut Active, mut followed: bool, first: &mut Option<Response>) -> Result<bool, EngineError> {
    let (mut attempt, mut restarts, mut replaced) = (0u32, 0u32, false);
    loop {
        // A server counting requests: none before the time it said (`Retry-After`).
        let hold = ctx.pace.hold();
        if ctx.pace.counts_requests() && !hold.is_zero() && pause(&ctx.stop, hold).await {
            return Ok(true);
        }
        let before = slot.pos.load(Acquire);
        let result = fetch(ctx, slot, followed, first.take()).await;
        followed = false;
        let err = match result {
            Ok(()) => return Ok(true),
            // A pause racing a network error is still a pause, not a failure.
            Err(_) if ctx.stop.is_cancelled() => return Ok(true),
            Err(e) if e.is_permanent() => return Err(e),
            Err(e) => e,
        };
        if slot.pos.load(Acquire) > before {
            attempt = 0;
        }
        // A dead or far too slow connection was closed (see `Slots::measure`): a fresh one takes
        // over at once — the bad one is never reused. Twice in a row is treated like any failure.
        if matches!(err, EngineError::Stalled) && !std::mem::replace(&mut replaced, true) {
            continue;
        }
        replaced = false;
        match &err {
            // A refusal was counted by `fetch` (with the server's `Retry-After`); a dead connection
            // is one bad path, not a sign that the network takes fewer connections.
            e if e.is_throttled() => {}
            EngineError::Stalled => {}
            _ => ctx.pace.trouble(ctx.progress.active.load(Acquire)),
        }
        err.forget_address();
        if ctx.ranges && active.try_leave() {
            ctx.slots.release(slot.clone());
            return Ok(false);
        }
        // Last connection standing: rides out Wi-Fi drops, sleeping laptops and busy servers, and
        // gives up only when nothing has arrived for a long while.
        if ctx.pace.stalled() {
            return Err(err);
        }
        if !ctx.ranges {
            restarts += 1;
            if restarts > MAX_RESTARTS {
                return Err(err);
            }
            restart(ctx, slot).await?;
        }
        attempt += 1;
        // The server said when to come back (`Retry-After`): exactly then. Otherwise longer and
        // longer waits.
        let base: u64 = if err.is_throttled() { 2000 } else { 500 };
        let hold = ctx.pace.hold();
        let wait = if err.is_throttled() && !hold.is_zero() { hold } else { Duration::from_millis(base << attempt.min(6)).min(MAX_BACKOFF).max(hold) };
        if pause(&ctx.stop, wait).await {
            return Ok(true);
        }
        // Refused (or a server counting requests): the next request starts at the first missing
        // byte and runs on through the free pieces — as few requests as possible.
        if ctx.ranges && (ctx.pace.counts_requests() || (ctx.pace.spare_requests() && err.is_throttled())) {
            *slot = ctx.slots.restart_lowest(slot);
        }
    }
}

/// No range support: the only way on after a cut is from the start.
async fn restart(ctx: &Ctx, slot: &Slot) -> std::io::Result<()> {
    slot.pos.store(0, Release);
    ctx.progress.downloaded.store(0, Relaxed);
    ctx.disk.truncate().await
}

/// Sleeps `d`; `true` if cancelled meanwhile.
pub(super) async fn pause(stop: &CancellationToken, d: Duration) -> bool {
    tokio::select! {
        () = stop.cancelled() => true,
        () = tokio::time::sleep(d) => false,
    }
}

async fn fetch(ctx: &Ctx, slot: &Slot, followed: bool, answered: Option<Response>) -> Result<(), EngineError> {
    let pos = slot.pos.load(Acquire);
    let end = slot.end.load(Acquire);
    if pos > end {
        return Ok(());
    }
    // Cancelled when this connection stops receiving while the others do (see `Slots::measure`).
    let kill = slot.attach();
    let asked_at = Instant::now();

    // A request that may run on into the next free piece, without asking again: whenever the
    // server counts requests, and for the first piece — should the server refuse the other
    // connections, this one carries on alone.
    let open = ctx.ranges && ctx.size.is_some() && (ctx.pace.spare_requests() || pos == 0);
    let asked = if open { ctx.size.map_or(end, |size| size - 1) } else { end };
    let res = match answered {
        // The download's first request (`bytes=0-`), answered already.
        Some(res) => res,
        None => {
            let mut req = ctx.client.get(ctx.url.clone()).headers(ctx.headers.clone());
            if ctx.ranges {
                req = req.header(RANGE, format!("bytes={pos}-{asked}"));
            }
            // Connecting can take seconds: a pause must not wait for it.
            let res = tokio::select! {
                biased;
                () = ctx.stop.cancelled() => return Ok(()),
                () = kill.cancelled() => return Err(EngineError::Stalled),
                res = req.send() => res?,
            };
            if matches!(res.status().as_u16(), 429 | 503) {
                ctx.pace.refuse(ctx.progress.active.load(Acquire), crate::retry_after(res.headers()), followed);
            }
            res.error_for_status()?
        }
    };
    // A 206 for another range (broken proxy/CDN), or from another version of the file (another
    // size: a mirror serving a newer release), would silently corrupt the file. A 200 is the whole
    // file: right only when the whole file was asked for (RFC 9110 lets a server answer that way).
    let whole_file = pos == 0 && ctx.size.is_some_and(|size| asked.checked_add(1) == Some(size));
    if let (Some(expected), Some(actual)) = (ctx.version.as_deref(), crate::probe::version_of(res.headers()))
        && expected
            .split_once(':')
            .zip(actual.split_once(':'))
            .is_some_and(|((kind, value), (other_kind, other_value))| kind == other_kind && value != other_value)
    {
        return Err(EngineError::FileChanged);
    }
    let received_range = crate::content_range(res.headers());
    let fits = match res.status() {
        StatusCode::PARTIAL_CONTENT => received_range.is_some_and(|(start, end, total)| {
            start == pos && end <= asked && total.is_none_or(|total| Some(total) == ctx.size) && res.content_length().is_none_or(|len| len == end - start + 1)
        }),
        // Its length too: a server answering with another version of the file (another size) would
        // not fit.
        StatusCode::OK => whole_file && res.content_length() == ctx.size,
        _ => false,
    };
    if ctx.ranges && !fits {
        return Err(EngineError::RangeIgnored);
    }
    let _receiving = Counted::enter(&ctx.streaming);
    let response_len = (res.status() == StatusCode::PARTIAL_CONTENT).then_some(received_range).flatten().map(|(start, end, _)| end - start + 1);
    let mut received = 0u64;

    let mut stream = res.bytes_stream();
    // Grows with the connection's speed up to its share: slow connections (flushed every few
    // seconds) never hold the full size.
    let mut buf = Vec::with_capacity(MIN_BUF.min(ctx.buf));
    let mut flushed = Instant::now();
    // Hot loop (thousands of chunks per second per connection): the stop token — shared by every
    // connection of the job — is subscribed to once, not re-registered under its lock per chunk.
    let stopped = ctx.stop.cancelled();
    let killed = kill.cancelled();
    tokio::pin!(stopped, killed);

    let result = loop {
        let chunk = tokio::select! {
            biased;
            () = &mut stopped => break Ok(false),
            () = &mut killed => break Err(EngineError::Stalled),
            chunk = stream.next() => chunk,
        };
        let chunk = match chunk {
            Some(Ok(c)) => c,
            Some(Err(e)) => break Err(EngineError::from(e)),
            None => break Ok(true),
        };
        received = received.saturating_add(chunk.len() as u64);
        if response_len.is_some_and(|len| received > len) {
            break Err(EngineError::RangeIgnored);
        }
        // What fits in this piece — for an open request, running on into the next free pieces.
        let (mut rest, mut taken, mut over) = (&chunk[..], 0, false);
        while !rest.is_empty() {
            let written = slot.pos.load(Acquire) + buf.len() as u64;
            let room = slot.end.load(Acquire).saturating_add(1).saturating_sub(written);
            if room == 0 {
                // A server counting requests: rather read again a piece already here than ask anew.
                match ctx.slots.run_on(slot, open, ctx.pace.counts_requests()) {
                    Some(again) => {
                        ctx.progress.downloaded.fetch_sub(again, Relaxed);
                        continue;
                    }
                    None => {
                        over = true;
                        break;
                    }
                }
            }
            let n = min(rest.len() as u64, room) as usize;
            if buf.len() + n > buf.capacity() {
                // Doubles towards this connection's share, never beyond it (plus this chunk).
                let target = (buf.capacity() * 2).clamp(MIN_BUF, ctx.buf).max(buf.len() + n);
                buf.reserve_exact(target - buf.len());
            }
            buf.extend_from_slice(&rest[..n]);
            slot.head.store(written + n as u64, Release);
            rest = &rest[n..];
            taken += n;
        }
        let n = taken;
        ctx.progress.downloaded.fetch_add(n as u64, Relaxed);
        ctx.pace.progressed(); // something arrived: the connection is alive, however slow
        // Throttling sleeps can be long: they must not delay a pause/shutdown. The global limit, then
        // this download's own.
        for limit in [&ctx.limit, &ctx.own_limit] {
            if limit.get() > 0 {
                tokio::select! {
                    biased;
                    () = &mut stopped => {}
                    () = limit.take(n) => {}
                }
            }
        }
        if over {
            break Ok(false);
        }
        // Full, or held for a while (a slow connection): to disk, where a checkpoint can count it.
        if buf.len() >= ctx.buf || flushed.elapsed() >= FLUSH_AFTER {
            buf = flush(&ctx.disk, buf, slot).await?;
            flushed = Instant::now();
        }
    };
    if !buf.is_empty() {
        flush(&ctx.disk, buf, slot).await?;
    }

    let eof = result?;
    let written = slot.pos.load(Acquire);
    // The piece is done: how fast this connection went (for telling stragglers apart).
    if slot.snapshot().is_done() {
        ctx.slots.finished(written.saturating_sub(pos), asked_at.elapsed());
    }
    match (eof, ctx.ranges, ctx.size) {
        (false, ..) => Ok(()),
        (true, false, _) if written == 0 => Err(EngineError::Empty),
        // Unknown length: the end of the stream is the end of the file.
        (true, false, None) => {
            slot.end.store(written - 1, Release);
            ctx.progress.total.store(written, Relaxed);
            Ok(())
        }
        // Known length: stopping short is a truncated file, never a success.
        (true, ..) if !slot.snapshot().is_done() => Err(EngineError::Truncated),
        (true, ..) => Ok(()),
    }
}

/// Writes the buffer where it belongs, then advances the segment: `pos` only ever covers bytes the
/// OS has, so a checkpoint (sync + state) never claims data still in flight. The buffer comes back
/// empty, for reuse.
async fn flush(disk: &Arc<Disk>, buf: Vec<u8>, slot: &Slot) -> std::io::Result<Vec<u8>> {
    let len = buf.len() as u64;
    let mut buf = disk.write_at(buf, slot.pos.load(Acquire)).await?;
    slot.pos.fetch_add(len, Release);
    buf.clear();
    Ok(buf)
}

/// Counts itself in an `AtomicUsize` while alive.
struct Counted<'a>(&'a AtomicUsize);

impl<'a> Counted<'a> {
    fn enter(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, AcqRel);
        Self(count)
    }
}

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, AcqRel);
    }
}

/// One live connection of a download, counted in `Progress::active` (shown, and read by the pacer).
pub(super) struct Active {
    ctx: Arc<Ctx>,
    left: bool,
}

impl Active {
    pub(super) fn enter(ctx: Arc<Ctx>) -> Self {
        ctx.progress.active.fetch_add(1, AcqRel);
        Self { ctx, left: false }
    }

    /// Leaves only if another connection stays active, so the last one never gives up.
    fn try_leave(&mut self) -> bool {
        // A compare-and-swap loop: `fetch_update` is deprecated from Rust 1.99, and its successor
        // (`try_update`) does not exist in the oldest Rust RDM builds with (`rust-version`).
        let active = &self.ctx.progress.active;
        let mut n = active.load(Acquire);
        self.left = loop {
            if n <= 1 {
                break false;
            }
            match active.compare_exchange_weak(n, n - 1, AcqRel, Acquire) {
                Ok(_) => break true,
                Err(now) => n = now,
            }
        };
        self.left
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        if !self.left {
            self.ctx.progress.active.fetch_sub(1, AcqRel);
        }
    }
}
