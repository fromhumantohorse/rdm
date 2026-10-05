//! Adaptive connection count, the way TCP adapts its window (AIMD): a download starts with a few
//! connections, doubles them every second while all of them stay healthy, then adds one at a time
//! once trouble has been seen; when connections fail (timeouts, resets, a server refusing more
//! with 429/503) it drops a quarter of them — half for an explicit refusal. A weak Wi-Fi, a
//! saturated router or a picky server thus settle on what they can take, instead of every
//! connection timing out ("timed out") and the download failing.

use std::{
    collections::HashMap,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

/// Connections opened at first; more join while the server and the network keep up.
pub(crate) const INITIAL_CONNECTIONS: usize = 8;
/// After a connection failed, no connection is added for this long.
const COOLDOWN: Duration = Duration::from_secs(8);
/// A burst of failures (the network dropped) counts once, not once per connection.
const CUT_SPACING: Duration = Duration::from_secs(2);
/// The last connection gives up only once nothing has arrived for this long (network down,
/// server gone); the download then fails with its resume point kept, and the app retries later.
pub(crate) const STALL_LIMIT: Duration = Duration::from_secs(180);
/// A server that refused connections (429/503) is asked for one more after this long at first,
/// then twice as long after each new refusal, up to `MAX_PROBE_WAIT`.
const PROBE_WAIT: Duration = Duration::from_secs(15);
const MAX_PROBE_WAIT: Duration = Duration::from_secs(300);
/// Longest `Retry-After` obeyed (a server asking for hours is asked again sooner).
const MAX_HOLD: Duration = Duration::from_secs(60);
/// How long the connection count a server accepted is remembered (other downloads, retries).
const MEMORY: Duration = Duration::from_secs(30 * 60);
const MEMORY_MAX_HOSTS: usize = 256;

pub(crate) struct Pace {
    max: usize,
    limit: AtomicUsize,
    epoch: Instant,
    /// Milliseconds since `epoch`, plus one (0 = never).
    last_trouble: AtomicU64,
    last_cut: AtomicU64,
    last_progress: AtomicU64,
    /// Most connections the server accepted: refusals (429/503) above it (`usize::MAX`: none).
    ceiling: AtomicUsize,
    /// Requests refused right after the same connection had one accepted…
    refused_after_accepted: AtomicU64,
    /// …twice: the server counts requests, not connections.
    counts_requests: AtomicBool,
    /// A refusal came: the next tick sets `ceiling` to the connections still receiving then.
    refused: AtomicBool,
    /// When one more connection may be tried above `ceiling`, and the wait after the next refusal.
    probe_at: AtomicU64,
    probe_wait: AtomicU64,
    /// No new request before this (the server's `Retry-After`).
    hold_until: AtomicU64,
    /// Where the ceiling is remembered (`host:port`).
    host: Option<String>,
    #[cfg(test)]
    travelled: AtomicU64,
}

impl Pace {
    /// `host`: the server, whose accepted connection count is shared with other downloads.
    pub fn new(max: usize, host: Option<String>) -> Self {
        let max = max.max(1);
        let known = host.as_deref().and_then(recall).unwrap_or_default();
        let (ceiling, counts_requests) = (known.accepted, known.counts_requests);
        let pace = Self {
            max,
            limit: AtomicUsize::new(max.min(INITIAL_CONNECTIONS).min(ceiling)),
            epoch: Instant::now(),
            last_trouble: AtomicU64::new(0),
            last_cut: AtomicU64::new(0),
            last_progress: AtomicU64::new(0),
            ceiling: AtomicUsize::new(ceiling),
            refused_after_accepted: AtomicU64::new(0),
            counts_requests: AtomicBool::new(counts_requests),
            refused: AtomicBool::new(false),
            probe_at: AtomicU64::new(millis(PROBE_WAIT)),
            probe_wait: AtomicU64::new(millis(PROBE_WAIT)),
            hold_until: AtomicU64::new(0),
            host,
            #[cfg(test)]
            travelled: AtomicU64::new(0),
        };
        // A recent download was told to come back later: this one too.
        if let Some(wait) = known.hold_until.and_then(|until| until.checked_duration_since(Instant::now())) {
            pace.hold_until.store(pace.now() + millis(wait), Relaxed);
        }
        pace.progressed(); // the stall clock starts now
        pace
    }

    fn now(&self) -> u64 {
        let now = u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX - 1) + 1;
        #[cfg(test)]
        let now = now + self.travelled.load(Relaxed);
        now
    }

    #[cfg(test)]
    fn travel(&self, by: Duration) {
        self.travelled.fetch_add(millis(by), Relaxed);
    }

    /// Connections allowed right now.
    pub fn limit(&self) -> usize {
        self.limit.load(Relaxed)
    }

    /// A connection failed (timeout, reset…) while `active` were open. A refusal by the server is
    /// [`Self::refuse`].
    pub fn trouble(&self, active: usize) {
        let now = self.now();
        self.last_trouble.store(now, Relaxed);
        let last = self.last_cut.load(Relaxed);
        if last != 0 && now.saturating_sub(last) < millis(CUT_SPACING) {
            return;
        }
        if self.last_cut.compare_exchange(last, now, Relaxed, Relaxed).is_ok() {
            self.limit.fetch_min((active * 3 / 4).max(1), Relaxed);
        }
    }

    /// The server refused a request (429/503) while `active` connections were open, maybe saying
    /// when to come back (`Retry-After`): no new connection meanwhile, and the next tick learns how
    /// many connections it accepts (those still receiving then). `followed`: the request came
    /// right after an accepted one on the same connection — a server limiting connections takes it
    /// (the count did not grow), one counting requests does not.
    pub fn refuse(&self, active: usize, retry_after: Option<Duration>, followed: bool) {
        let now = self.now();
        self.last_trouble.store(now, Relaxed);
        self.refused.store(true, Relaxed);
        // Twice: once may be a race with a connection added at the same moment.
        if followed && self.refused_after_accepted.fetch_add(1, Relaxed) >= 1 {
            self.counts_requests.store(true, Relaxed);
        }
        self.limit.fetch_min(active.saturating_sub(1).max(1), Relaxed);
        if let Some(wait) = retry_after.map(|w| w.min(MAX_HOLD)) {
            self.hold_until.fetch_max(now + millis(wait), Relaxed);
            if let Some(host) = &self.host {
                let until = Instant::now() + wait;
                remember(host, |k| k.hold_until = Some(k.hold_until.map_or(until, |t| t.max(until))));
            }
        }
    }

    /// How long no new request may be sent yet (the server's `Retry-After`).
    pub fn hold(&self) -> Duration {
        Duration::from_millis(self.hold_until.load(Relaxed).saturating_sub(self.now()))
    }

    /// Nothing has gone wrong yet: connections double at each tick while they pay off.
    pub fn slow_start(&self) -> bool {
        self.last_trouble.load(Relaxed) == 0
    }

    /// The server refused connections (now or in a recent download): requests are spared — free
    /// pieces are joined, and a request runs on into the next free piece (see `transfer`).
    pub fn spare_requests(&self) -> bool {
        self.ceiling.load(Relaxed) != usize::MAX
    }

    /// The server refused requests without more connections than it took before (now or in a
    /// recent download): it counts requests — reading again a piece already here beats asking.
    pub fn counts_requests(&self) -> bool {
        self.counts_requests.load(Relaxed)
    }

    /// Called at each tick with the connections receiving data (`streaming`): raises the limit
    /// if the connections have been healthy and more of them still pay off (`grow`, see
    /// [`Growth`]), never above `cap` (see [`speed_cap`]) nor — but for one more now and then —
    /// above what the server accepted.
    pub fn ramp(&self, cap: usize, grow: bool, streaming: usize) -> usize {
        let now = self.now();
        if self.refused.swap(false, Relaxed) {
            let accepted = streaming.max(1);
            self.ceiling.store(accepted, Relaxed);
            self.limit.fetch_min(accepted, Relaxed);
            let wait = self.probe_wait.load(Relaxed);
            self.probe_at.store(now + wait, Relaxed);
            self.probe_wait.store(wait.saturating_mul(2).min(millis(MAX_PROBE_WAIT)), Relaxed);
            if let Some(host) = &self.host {
                let counts = self.counts_requests();
                remember(host, |k| (k.accepted, k.counts_requests) = (accepted, counts));
            }
        }
        let limit = self.limit.load(Relaxed);
        if now < self.hold_until.load(Relaxed) {
            return limit;
        }
        let trouble = self.last_trouble.load(Relaxed);
        let mut next = if !grow {
            limit
        } else if trouble == 0 {
            limit.saturating_mul(2) // slow start: nothing went wrong yet
        } else if now.saturating_sub(trouble) >= millis(COOLDOWN) {
            limit + 1
        } else {
            limit
        };
        let ceiling = self.ceiling.load(Relaxed);
        if next > ceiling {
            next = ceiling;
            // One more than the server accepted, now and then: it may take more now. Not for a
            // server counting requests: each try would cost a refusal and its wait.
            if grow && limit >= ceiling && ceiling < self.max && !self.counts_requests() && now >= self.probe_at.load(Relaxed) {
                next = ceiling + 1;
                self.ceiling.store(next, Relaxed);
                self.probe_at.store(now + self.probe_wait.load(Relaxed), Relaxed);
                if let Some(host) = &self.host {
                    remember(host, |k| k.accepted = next);
                }
            }
        }
        let next = next.clamp(1, self.max.min(cap).max(1));
        self.limit.store(next, Relaxed);
        next
    }

    /// Lowers the limit to `cap` right away (a speed limit was set).
    pub fn cap(&self, cap: usize) {
        self.limit.fetch_min(cap.max(1), Relaxed);
    }

    /// Bytes arrived.
    pub fn progressed(&self) {
        self.last_progress.store(self.now(), Relaxed);
    }

    /// Nothing has arrived for `STALL_LIMIT`.
    pub fn stalled(&self) -> bool {
        self.now().saturating_sub(self.last_progress.load(Relaxed)) > millis(STALL_LIMIT)
    }
}

/// Whether adding connections still pays off, judged on the measured throughput (hill climbing):
/// after each increase it waits for the new connections to get going, then compares. A line that
/// is already full (a slow ADSL, a Wi-Fi at its limit) stops growing — extra connections would only
/// add overhead and unfairness to other traffic — and a new attempt is made every so often, in
/// case the network got faster.
#[derive(Default)]
pub(crate) struct Growth {
    /// Bytes received during the last two ticks.
    recent: [u64; 2],
    /// Throughput before the last increase.
    baseline: Option<u64>,
    /// Ticks left before judging the last increase.
    settle: u8,
    /// Increases in a row that did not help.
    misses: u8,
    /// Ticks spent on the current plateau.
    plateau: u16,
}

// Ticks of half a second (see `transfer`).
/// Ticks given to new connections (handshakes, TCP slow start) before judging them: 2 s.
const SETTLE_TICKS: u8 = 4;
/// Ticks without improvement before calling it a plateau (throughput is noisy): 3 s.
const PATIENCE: u8 = 6;
/// A plateau is tried again after this many ticks: 15 s.
const REPROBE_TICKS: u16 = 30;

impl Growth {
    /// Called once per tick with the bytes received during it; `true` when more connections may help.
    /// `quick` (slow start, nothing went wrong yet): each tick is judged on its own, right after
    /// the last increase — connections double every second while they pay off.
    pub fn more(&mut self, bytes: u64, quick: bool) -> bool {
        // The very first tick has no predecessor: not averaged with a zero.
        self.recent = if self.baseline.is_none() && self.settle == 0 { [bytes, bytes] } else { [self.recent[1], bytes] };
        if self.settle > 0 {
            self.settle -= 1;
            return false;
        }
        let rate = if quick { bytes } else { (self.recent[0] + self.recent[1]) / 2 };
        let improved = self.baseline.is_none_or(|before| rate >= before.saturating_add(before / 10).max(1));
        if !improved {
            self.misses = self.misses.saturating_add(1);
            if self.misses < PATIENCE {
                return false;
            }
            // Plateau: hold, and probe once more after a while.
            self.plateau += 1;
            if self.plateau < REPROBE_TICKS {
                return false;
            }
        }
        self.baseline = Some(rate);
        self.misses = 0;
        self.plateau = 0;
        self.settle = if quick { 0 } else { SETTLE_TICKS };
        true
    }
}

/// What is known of a server, shared by every download (and its retries): a new download from a
/// server that refuses more than 4 connections starts with 4, instead of being refused (and maybe
/// penalized) again.
#[derive(Debug, Clone, Copy)]
struct Known {
    /// The connections it accepted (`usize::MAX`: no refusal seen).
    accepted: usize,
    counts_requests: bool,
    /// It asked to come back then (`Retry-After`).
    hold_until: Option<Instant>,
    at: Instant,
}

impl Known {
    fn new(at: Instant) -> Self {
        Self { accepted: usize::MAX, counts_requests: false, hold_until: None, at }
    }

    fn fresh(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.at) < MEMORY
    }
}

impl Default for Known {
    fn default() -> Self {
        Self::new(Instant::now())
    }
}

/// The servers met recently, at most `MEMORY_MAX_HOSTS` of them.
#[derive(Default)]
struct Servers(HashMap<String, Known>);

impl Servers {
    fn recall(&self, host: &str, now: Instant) -> Option<Known> {
        self.0.get(host).filter(|k| k.fresh(now)).copied()
    }

    fn remember(&mut self, host: &str, now: Instant, change: impl FnOnce(&mut Known)) {
        if self.0.len() >= MEMORY_MAX_HOSTS && !self.0.contains_key(host) {
            self.0.retain(|_, k| k.fresh(now));
            if self.0.len() >= MEMORY_MAX_HOSTS {
                self.0.clear();
            }
        }
        let known = self.0.entry(host.to_owned()).or_insert_with(|| Known::new(now));
        if !known.fresh(now) {
            *known = Known::new(now);
        }
        change(known);
        known.at = now;
    }
}

fn servers() -> std::sync::MutexGuard<'static, Servers> {
    static SERVERS: std::sync::OnceLock<Mutex<Servers>> = std::sync::OnceLock::new();
    SERVERS.get_or_init(Mutex::default).lock().unwrap_or_else(PoisonError::into_inner)
}

fn recall(host: &str) -> Option<Known> {
    servers().recall(host, Instant::now())
}

fn remember(host: &str, change: impl FnOnce(&mut Known)) {
    servers().remember(host, Instant::now(), change);
}

/// Under a speed limit (bytes per second, 0 = none), more connections only share the same bytes
/// and wait longer for their turn — long enough, with many of them, for the server to time them
/// out: one connection per 256 KiB/s allowed.
pub(crate) fn speed_cap(limit: u64) -> usize {
    if limit == 0 { usize::MAX } else { usize::try_from(limit / (256 << 10)).unwrap_or(usize::MAX).max(1) }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_small_and_doubles_while_healthy() {
        let pace = Pace::new(32, None);
        assert_eq!(pace.limit(), INITIAL_CONNECTIONS);
        assert_eq!(pace.ramp(usize::MAX, true, 8), 16);
        assert_eq!(pace.ramp(usize::MAX, true, 16), 32);
        assert_eq!(pace.ramp(usize::MAX, true, 32), 32, "never above the configured maximum");
        assert_eq!(Pace::new(3, None).limit(), 3);
    }

    #[test]
    fn backs_off_on_trouble_and_holds_during_cooldown() {
        let pace = Pace::new(32, None);
        pace.ramp(usize::MAX, true, 8);
        pace.trouble(16);
        assert_eq!(pace.limit(), 12, "a quarter fewer");
        pace.trouble(12);
        assert_eq!(pace.limit(), 12, "a burst of failures is one event");
        assert_eq!(pace.ramp(usize::MAX, true, 12), 12, "no new connection right after trouble");
    }

    /// A server that takes 4 connections and refuses the others (429): the count settles on the
    /// connections that kept receiving, and is not exceeded until the probe wait is over.
    #[test]
    fn learns_how_many_connections_a_server_accepts() {
        let pace = Pace::new(32, None);
        for _ in 0..4 {
            pace.refuse(8, None, false);
        }
        assert!(pace.limit() < 8, "no new connection after a refusal");
        assert_eq!(pace.ramp(usize::MAX, true, 4), 4, "the 4 connections still receiving");
        for _ in 0..5 {
            assert_eq!(pace.ramp(usize::MAX, true, 4), 4, "not above what the server accepted");
        }
        let single = Pace::new(1, None);
        single.refuse(1, None, false);
        assert_eq!(single.limit(), 1, "never zero");
    }

    /// Refused right after an accepted request on the same connection: the server counts requests
    /// (OVH); refused only when opening more: it counts connections.
    #[test]
    fn tells_a_request_limit_from_a_connection_limit() {
        let requests = Pace::new(32, None);
        requests.refuse(8, None, false);
        requests.ramp(usize::MAX, true, 3);
        assert!(!requests.counts_requests() && requests.spare_requests(), "a first refusal says little yet");
        requests.refuse(3, None, true);
        assert!(!requests.counts_requests(), "once may be a race");
        requests.refuse(2, None, true);
        assert!(requests.counts_requests());

        let connections = Pace::new(32, None);
        connections.refuse(8, None, false);
        connections.ramp(usize::MAX, true, 4);
        connections.refuse(5, None, false);
        connections.refuse(5, None, true);
        assert!(!connections.counts_requests(), "refused only when opening one more");
    }

    #[test]
    fn obeys_retry_after() {
        let pace = Pace::new(32, None);
        pace.refuse(8, Some(Duration::from_secs(10)), false);
        assert!(pace.hold() > Duration::from_secs(9));
        let limit = pace.limit();
        assert_eq!(pace.ramp(usize::MAX, true, 1), 1, "the ceiling is learned even while holding");
        assert!(limit >= 1);
        let long = Pace::new(32, None);
        long.refuse(8, Some(Duration::from_secs(3600)), false);
        assert!(long.hold() <= MAX_HOLD, "a server asking for an hour is asked again sooner");
    }

    #[test]
    fn a_learned_count_applies_to_the_next_download_from_that_server() {
        let host = "cap.example:443".to_owned();
        let first = Pace::new(32, Some(host.clone()));
        first.refuse(8, None, false);
        first.ramp(usize::MAX, true, 3);
        assert_eq!(Pace::new(32, Some(host)).limit(), 3);
        assert_eq!(Pace::new(32, Some("other.example:443".into())).limit(), INITIAL_CONNECTIONS);
    }

    #[test]
    fn a_wait_asked_by_a_server_applies_to_its_next_download() {
        let host = "busy.example:443".to_owned();
        Pace::new(32, Some(host.clone())).refuse(8, Some(Duration::from_secs(10)), false);
        assert!(Pace::new(32, Some(host)).hold() > Duration::from_secs(9));
        assert!(Pace::new(32, Some("calm.example:443".into())).hold().is_zero());
    }

    #[test]
    fn a_speed_limit_caps_connections() {
        assert_eq!(speed_cap(0), usize::MAX);
        assert_eq!(speed_cap(10 << 10), 1, "10 KiB/s: one connection");
        assert_eq!(speed_cap(1 << 20), 4);
        let pace = Pace::new(32, None);
        pace.cap(speed_cap(512 << 10));
        assert_eq!(pace.limit(), 2);
        assert_eq!(pace.ramp(speed_cap(512 << 10), true, 2), 2);
        assert_eq!(pace.ramp(usize::MAX, true, 2), 4, "limit lifted: growing again");
    }

    #[test]
    fn keeps_growing_while_throughput_rises() {
        for quick in [false, true] {
            let mut g = Growth::default();
            let mut rate = 1_000_000;
            let mut increases = 0;
            for _ in 0..60 {
                if g.more(rate, quick) {
                    increases += 1;
                    rate = rate * 3 / 2; // every increase pays off
                }
            }
            assert!(increases >= 8, "{increases}");
        }
    }

    #[test]
    fn slow_start_judges_every_tick() {
        let mut g = Growth::default();
        let mut rate = 1_000_000;
        let when: Vec<usize> = (0..4)
            .filter(|_| {
                let more = g.more(rate, true);
                rate *= 2;
                more
            })
            .collect();
        assert_eq!(when, [0, 1, 2, 3], "doubling pays off at every tick");
    }

    /// A line already full: an increase, 2 s for the new connections to settle, 3 s of patience,
    /// then 15 s on the plateau before the next try — 39 ticks of half a second in all.
    #[test]
    fn a_full_line_is_tried_again_after_settling_patience_and_a_pause() {
        let mut growth = Growth::default();

        let tries: Vec<usize> = (0..120).filter(|_| growth.more(5_000_000, false)).collect();

        assert_eq!(tries, [0, 39, 78, 117]);
    }

    /// Outside the slow start a single good tick is not enough: the last two are averaged.
    #[test]
    fn growth_is_judged_on_the_last_two_ticks() {
        let mut growth = Growth::default();
        assert!(growth.more(100, false), "the first increase");
        for _ in 0..SETTLE_TICKS {
            assert!(!growth.more(100, false), "settling");
        }

        assert!(!growth.more(115, false), "(100 + 115) / 2 is less than 10 % better");
    }

    #[test]
    fn a_new_download_starts_slowly_and_spares_no_request() {
        let pace = Pace::new(32, None);
        assert!(pace.slow_start() && !pace.spare_requests() && !pace.counts_requests());

        pace.trouble(8);

        assert!(!pace.slow_start(), "trouble ends the slow start");
    }

    #[test]
    fn after_trouble_one_connection_is_added_once_the_cooldown_is_over() {
        let pace = Pace::new(32, None);
        pace.trouble(8);
        assert_eq!(pace.ramp(usize::MAX, true, 6), 6, "cooling down");

        pace.travel(COOLDOWN);

        assert_eq!(pace.ramp(usize::MAX, true, 6), 7, "one at a time from now on");
    }

    /// A server that took 4 connections: kept to 4, then one more is tried now and then, each try
    /// waiting twice as long as the one before.
    #[test]
    fn one_more_connection_is_tried_once_in_a_while() {
        let pace = Pace::new(32, None);
        pace.travel(Duration::from_secs(1));
        pace.refuse(8, None, false);
        assert_eq!(pace.ramp(usize::MAX, true, 4), 4);

        pace.travel(COOLDOWN);
        assert_eq!(pace.ramp(usize::MAX, true, 4), 4, "cooled down, but the server took 4");

        pace.travel(PROBE_WAIT - COOLDOWN);
        assert_eq!(pace.ramp(usize::MAX, true, 4), 5);
        assert_eq!(pace.ramp(usize::MAX, true, 5), 5, "the next try waits");

        pace.travel(2 * PROBE_WAIT);
        assert_eq!(pace.ramp(usize::MAX, true, 5), 6);
    }

    #[test]
    fn a_server_counting_requests_is_never_asked_for_one_more() {
        let pace = Pace::new(32, None);
        pace.refuse(8, None, true);
        pace.refuse(8, None, true);
        assert!(pace.counts_requests());
        assert_eq!(pace.ramp(usize::MAX, true, 4), 4);

        pace.travel(2 * PROBE_WAIT);

        assert_eq!(pace.ramp(usize::MAX, true, 4), 4, "each try would cost a refusal and its wait");
    }

    #[test]
    fn no_connection_is_tried_above_the_configured_maximum() {
        let host = "full.example:443".to_owned();
        let pace = Pace::new(4, Some(host.clone()));
        pace.travel(Duration::from_secs(1));
        pace.refuse(4, None, false);
        pace.ramp(usize::MAX, true, 4);
        pace.travel(PROBE_WAIT);

        for _ in 0..3 {
            assert_eq!(pace.ramp(usize::MAX, true, 4), 4);
        }
        assert_eq!(Pace::new(32, Some(host)).limit(), 4, "no try above it remembered either");
    }

    #[test]
    fn a_wait_asked_in_a_recent_download_stops_growth_until_it_is_over() {
        let host = "patient.example:443".to_owned();
        Pace::new(32, Some(host.clone())).refuse(8, Some(Duration::from_secs(10)), false);
        let next = Pace::new(32, Some(host));
        assert_eq!(next.ramp(usize::MAX, true, 8), 8, "no new connection before the server's time");

        next.travel(Duration::from_secs(11));

        assert_eq!(next.ramp(usize::MAX, true, 8), 16, "then the slow start goes on");
    }

    #[test]
    fn the_servers_wait_is_counted_from_its_refusal() {
        let pace = Pace::new(32, None);
        pace.travel(Duration::from_secs(5));

        pace.refuse(8, Some(Duration::from_secs(10)), false);

        let hold = pace.hold();
        assert!(hold > Duration::from_secs(9) && hold <= Duration::from_secs(10), "{hold:?}");
    }

    #[test]
    fn the_last_connection_gives_up_only_after_three_minutes_without_data() {
        let pace = Pace::new(4, None);
        pace.travel(STALL_LIMIT - Duration::from_secs(1));
        assert!(!pace.stalled());

        pace.travel(Duration::from_secs(2));
        assert!(pace.stalled());

        pace.progressed();
        assert!(!pace.stalled(), "a byte arrived");
    }

    #[test]
    fn what_is_known_of_a_server_is_forgotten_after_half_an_hour() {
        let mut servers = Servers::default();
        let then = Instant::now();
        servers.remember("cdn.example:443", then, |k| k.accepted = 4);

        let accepted = |at: Instant| servers.recall("cdn.example:443", at).map(|k| k.accepted);

        assert_eq!(accepted(then + Duration::from_secs(29 * 60)), Some(4));
        assert_eq!(accepted(then + Duration::from_secs(30 * 60)), None);
    }

    #[test]
    fn a_server_remembered_again_after_half_an_hour_starts_afresh() {
        let mut servers = Servers::default();
        let then = Instant::now();
        servers.remember("cdn.example:443", then, |k| k.accepted = 4);

        let later = then + Duration::from_secs(31 * 60);
        servers.remember("cdn.example:443", later, |_| {});

        assert_eq!(servers.recall("cdn.example:443", later).map(|k| k.accepted), Some(usize::MAX));
    }

    #[test]
    fn a_full_memory_makes_room_by_forgetting_the_old_servers() {
        let mut servers = Servers::default();
        let then = Instant::now();
        for i in 0..MEMORY_MAX_HOSTS {
            servers.remember(&format!("old{i}.example:443"), then, |_| {});
        }

        let later = then + Duration::from_secs(31 * 60);
        servers.remember("new.example:443", later, |k| k.accepted = 2);

        assert_eq!(servers.0.len(), 1);
        assert_eq!(servers.recall("new.example:443", later).map(|k| k.accepted), Some(2));
    }

    #[test]
    fn a_full_memory_of_recent_servers_starts_over_for_a_new_one_only() {
        let mut servers = Servers::default();
        let now = Instant::now();
        for i in 0..MEMORY_MAX_HOSTS {
            servers.remember(&format!("busy{i}.example:443"), now, |_| {});
        }

        servers.remember("busy0.example:443", now, |k| k.accepted = 3);
        servers.remember("busy0.example:443", now, |_| {});
        assert_eq!(servers.0.len(), MEMORY_MAX_HOSTS, "a server already known takes no room");
        assert_eq!(servers.recall("busy0.example:443", now).map(|k| k.accepted), Some(3), "and keeps what was learned");

        servers.remember("new.example:443", now, |_| {});
        assert_eq!(servers.0.len(), 1);
    }

    #[test]
    fn holds_when_told_not_to_grow() {
        let pace = Pace::new(64, None);
        assert_eq!(pace.ramp(usize::MAX, false, 8), INITIAL_CONNECTIONS);
    }

    #[test]
    fn not_stalled_at_start() {
        assert!(!Pace::new(4, None).stalled());
    }
}
