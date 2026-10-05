//! The service's pulse: speeds and the last minute's chart, retries coming due, the automatic
//! switch to the proxy, and the list written now and then.

use super::*;

/// Speeds, idle recordings and the list on disk are refreshed at this pace.
const TICK: Duration = Duration::from_millis(500);
/// … and at this one once nothing has moved for `IDLE_AFTER` ticks.
const IDLE_TICK: Duration = Duration::from_secs(2);
const IDLE_AFTER: u32 = 4;
/// While transfers run, the list (with progress) is written every this many ticks (10 s).
const PROGRESS_SAVE_TICKS: u32 = 20;
/// Samples of the total speed kept for the chart: one minute.
pub(super) const HISTORY: usize = 120;

/// Automatic proxy mode: a download that runs below `SLOW_SPEED` (bytes/s) for `SLOW_FOR`, once
/// past its first `SLOW_GRACE`, switches to the proxy.
const SLOW_SPEED: f64 = 32.0 * 1024.0;
const SLOW_FOR: Duration = Duration::from_secs(20);
const SLOW_GRACE: Duration = Duration::from_secs(15);

#[derive(Default, Clone, Copy)]
pub struct Stats {
    pub running: usize,
    pub queued: usize,
    pub speed: f64,
    /// Bytes done / known total over running downloads (for the tray tooltip).
    pub done: u64,
    pub total: u64,
}

impl Manager {
    pub fn stats(&self) -> Stats {
        lock(&self.entries).iter().fold(Stats::default(), |mut s, e| {
            match e.download.status() {
                Status::Running => {
                    let (done, total, _) = e.progress.snapshot();
                    s.running += 1;
                    s.speed += e.speed;
                    s.done += done.min(total);
                    s.total += total;
                }
                Status::Queued => s.queued += 1,
                _ => {}
            }
            s
        })
    }

    /// Total speed over the last minute, oldest first (bytes per second).
    pub fn speed_history(&self) -> Vec<f32> {
        lock(&self.history).iter().copied().collect()
    }

    pub(super) fn spawn_ticker(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.rt.spawn(async move {
            let (mut ticks, mut idle, mut last) = (0u32, 0u32, Instant::now());
            loop {
                // Nothing moving for a while: fewer wake-ups (a new download is started by the
                // scheduler itself, not by this tick).
                tokio::time::sleep(if idle >= IDLE_AFTER { IDLE_TICK } else { TICK }).await;
                let Some(this) = weak.upgrade() else { return };
                this.reap_idle_recordings();
                let elapsed = std::mem::replace(&mut last, Instant::now()).elapsed();
                let (moving, retry_due) = this.tick(elapsed);
                let recording = !lock(&this.recordings).is_empty();
                idle = if moving || retry_due || recording { 0 } else { idle.saturating_add(1) };
                if moving {
                    this.repaint();
                }
                if retry_due {
                    this.schedule();
                }
                // Progress written now and then while transfers run: after a crash the list shows
                // where each download was (the resume point itself lives in its `.rdm` file).
                ticks = ticks.wrapping_add(1);
                if moving && ticks.is_multiple_of(PROGRESS_SAVE_TICKS) {
                    this.dirty.store(true, Release);
                }
                if this.dirty.load(Acquire) {
                    let _ = tokio::task::spawn_blocking(move || this.persist()).await;
                }
            }
        });
    }

    /// Updates speeds and the history. Returns whether the UI has something moving to show (a
    /// running download, a retry countdown, the chart still scrolling back to zero), and whether
    /// a download waiting to retry is due.
    fn tick(&self, elapsed: Duration) -> (bool, bool) {
        let (mut active, mut total, mut due) = (false, 0.0, false);
        let now = Instant::now();
        let auto_proxy = self.auto_proxy();
        for e in lock(&self.entries).iter_mut() {
            let (done, total_size, _) = e.progress.snapshot();
            let running = *e.download.status() == Status::Running;
            let instant = done.saturating_sub(e.last) as f64 / elapsed.as_secs_f64().max(0.05);
            e.speed = if running { e.speed * 0.6 + instant * 0.4 } else { 0.0 };
            e.last = done;
            let waiting = *e.download.status() == Status::Queued && e.retry.is_some();
            active |= running || waiting || e.resolving;
            due |= waiting && e.startable(now);
            total += e.speed;
            // Automatic proxy mode: a download stuck slow on the direct route switches (it keeps
            // its progress). Not near its end: a nearly finished file is not worth a restart.
            let settled = e.started.is_some_and(|t| t.elapsed() > SLOW_GRACE);
            let far_from_done = total_size == 0 || done.saturating_mul(10) < total_size.saturating_mul(9);
            // Slow because the user capped it: the proxy would not help.
            let capped = engine::Job::effective_limit(&self.limit, &e.own_limit);
            let chosen = capped > 0 && (capped as f64) < SLOW_SPEED * 4.0;
            if auto_proxy && running && !e.via_proxy && !e.download.is_recording() && settled && far_from_done && !chosen && e.speed < SLOW_SPEED {
                let since = *e.slow_since.get_or_insert(now);
                if now.duration_since(since) >= SLOW_FOR {
                    e.via_proxy = true;
                    e.restart();
                }
            } else {
                e.slow_since = None;
            }
        }
        let mut history = lock(&self.history);
        history.pop_front();
        history.push_back(total as f32);
        (active || history.iter().any(|&s| s > 0.0), due)
    }
}
