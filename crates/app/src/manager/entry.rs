//! One download of the list: its state as the window shows it, what starts it, and what is saved.

use super::*;

/// Never written to disk: session cookies stay in memory only.
const SECRET_HEADERS: [&str; 3] = ["cookie", "authorization", "proxy-authorization"];

pub struct Entry {
    pub download: Download,
    pub progress: Arc<Progress>,
    pub speed: f64,
    /// SHA-256 of the finished file, once computed (on request, or for VirusTotal).
    pub sha256: Option<String>,
    /// VirusTotal analysis of the finished file.
    pub scan: Scan,
    /// Derived once from the (immutable) target: the list redraws them every frame.
    pub name: String,
    /// `name` in lower case, for the search box.
    pub search_key: String,
    pub category: Category,
    /// Just added: the server is being asked for the file's real name; not started before.
    pub resolving: bool,
    /// Waiting to retry after a transient failure, and why it failed.
    pub retry: Option<Retry>,
    /// Checksum verification of the finished file (see `checksum`).
    pub verify: Verify,
    /// Automatic proxy mode: this download goes through the proxy (slow or unreachable directly).
    pub via_proxy: bool,
    pub(super) headers: Vec<(String, String)>,
    pub(super) cancel: Option<CancellationToken>,
    pub(super) last: u64,
    /// Automatic retries in a row without progress.
    pub(super) retries: u32,
    /// When it was added in this session (`None`: loaded from disk).
    pub(super) added: Option<Instant>,
    /// Its own speed limiter, shared with its running job: a change applies at once.
    pub(super) own_limit: Arc<RateLimit>,
    /// When it last started, and since when it has been too slow (automatic proxy).
    pub(super) started: Option<Instant>,
    pub(super) slow_since: Option<Instant>,
    /// Stopped to start again right away (new route, new link, certificate choice).
    pub(super) restart: bool,
    /// Its target was an existing file, to overwrite ("overwrite" in the settings): removing the
    /// download must not take that file along before the transfer wrote over it.
    pub(super) replaces: bool,
}

/// Checking a finished file against the checksum the user gave.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Verify {
    #[default]
    None,
    Running,
    Ok,
    /// The file's actual checksum.
    Mismatch(String),
    Failed(String),
}

/// A download back in the queue after a transient failure (network down, busy server).
#[derive(Debug, Clone)]
pub struct Retry {
    pub at: Instant,
    pub reason: Failure,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Scan {
    #[default]
    None,
    Running(Stage),
    Done(Report),
    Failed(virustotal::Error),
}

/// Why a VirusTotal analysis cannot start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanRefused {
    /// No API key yet: the settings must ask for one.
    NoKey,
    /// Not finished, too large for VirusTotal, or already being analysed.
    NotEligible,
}

/// A download about to start: its job, and how to reach its server.
pub(super) struct Launch {
    pub(super) id: DownloadId,
    pub(super) job: engine::Job,
    pub(super) progress: Arc<Progress>,
    pub(super) cancel: CancellationToken,
    pub(super) via_proxy: bool,
    pub(super) insecure: bool,
}

impl Entry {
    pub(super) fn new(download: Download, headers: Vec<(String, String)>, downloaded: u64, total: u64) -> Self {
        let progress = Arc::new(Progress::default());
        progress.downloaded.store(downloaded, Relaxed);
        progress.total.store(total, Relaxed);
        let own_limit = Arc::new(RateLimit::default());
        own_limit.set(u64::from(download.speed_limit_kib) * 1024);
        let mut entry = Self {
            search_key: String::new(),
            name: String::new(),
            category: download.category(),
            download,
            progress,
            speed: 0.0,
            sha256: None,
            scan: Scan::None,
            resolving: false,
            retry: None,
            verify: Verify::None,
            via_proxy: false,
            headers,
            cancel: None,
            last: downloaded,
            retries: 0,
            added: None,
            own_limit,
            started: None,
            slow_since: None,
            restart: false,
            replaces: false,
        };
        entry.named();
        entry
    }

    /// Stops a running transfer to start it again at once (it keeps its progress).
    pub(super) fn restart(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            self.restart = true;
            cancel.cancel();
        }
    }

    /// Derives what the list shows from the target (after creation, or a rename before start).
    pub(super) fn named(&mut self) {
        let d = &self.download;
        self.name = d.target.file_name().map_or_else(|| d.url.to_string(), |n| n.to_string_lossy().into_owned());
        self.search_key = self.name.to_lowercase();
        self.category = d.category();
    }

    /// Seconds left at the current speed.
    pub fn eta(&self) -> Option<u64> {
        let (done, total, _) = self.progress.snapshot();
        (self.speed > 1.0 && total > done).then(|| ((total - done) as f64 / self.speed) as u64)
    }

    /// A finished file VirusTotal accepts (not empty, 650 MB at most).
    pub fn scannable(&self) -> bool {
        let size = self.progress.total.load(Relaxed).max(self.progress.downloaded.load(Relaxed));
        *self.download.status() == Status::Completed && (1..=virustotal::MAX_UPLOAD).contains(&size)
    }

    /// Quitting now (an automatic update) would interrupt it: it runs (a recording too) or waits in
    /// the queue, or its file is being verified or analysed — results kept in memory only.
    pub(super) fn keeps_rdm_open(&self) -> bool {
        matches!(self.download.status(), Status::Running | Status::Queued) || self.verify == Verify::Running || matches!(self.scan, Scan::Running(_))
    }

    /// Takes a download slot (browser recordings do not: the browser paces them).
    pub(super) fn occupies_slot(&self) -> bool {
        *self.download.status() == Status::Running && !self.download.is_recording()
    }

    /// A recording follows the browser's playback: it cannot be paused from here.
    pub(super) fn pause(&mut self) {
        if !self.download.is_recording() && self.download.pause().is_ok() {
            self.retry = None;
            if let Some(cancel) = self.cancel.take() {
                cancel.cancel();
            }
        }
    }

    /// The user asked: no waiting for an automatic retry, and a fresh retry budget.
    pub(super) fn resume(&mut self) {
        if self.download.enqueue().is_ok() || self.retry.is_some() {
            self.retry = None;
            self.retries = 0;
        }
    }

    /// Can the scheduler start it now?
    pub(super) fn startable(&self, now: Instant) -> bool {
        *self.download.status() == Status::Queued && !self.resolving && self.retry.as_ref().is_none_or(|r| r.at <= now)
    }

    /// Queued → running: the engine job, with a fresh cancel token.
    pub(super) fn start(&mut self, limit: &Arc<RateLimit>) -> Option<Launch> {
        self.download.start().ok()?;
        self.retry = None;
        self.restart = false;
        self.started = Some(Instant::now());
        self.slow_since = None;
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        let job = engine::Job {
            url: self.download.url.clone(),
            target: self.download.target.clone(),
            connections: self.download.connections(),
            headers: to_header_map(&self.headers),
            audio: self.download.audio.clone(),
            limit: limit.clone(),
            own_limit: self.own_limit.clone(),
        };
        Some(Launch {
            id: self.download.id,
            job,
            progress: self.progress.clone(),
            cancel,
            via_proxy: self.via_proxy,
            insecure: self.download.insecure,
        })
    }

    /// Finished: the session's secrets have no use left (a long history would keep them all).
    pub(super) fn forget_secrets(&mut self) {
        self.headers.retain(|(k, _)| !secret_header(k));
    }

    /// A new link for this download. What was granted to the old one does not follow it elsewhere:
    /// its raw cookies stay on their host, its authentication on its origin, and a certificate
    /// exemption on its host.
    pub(super) fn moved_to(&mut self, url: Url) {
        let allowed = self.headers_for(&url);
        self.headers.retain(|(k, _)| !secret_header(k) || allowed.contains_key(k.as_str()));
        self.download.insecure &= self.download.url.host_str() == url.host_str();
        self.download.url = url;
    }

    /// Its request headers, as fit for `url` (see `engine::net::headers_for`).
    pub(super) fn headers_for(&self, url: &Url) -> HeaderMap {
        engine::net::headers_for(&to_header_map(&self.headers), &self.download.url, url).into_owned()
    }

    /// What goes to disk: never the session secrets.
    pub(super) fn stored(&self) -> Stored {
        let (downloaded, total, _) = self.progress.snapshot();
        let headers = self.headers.iter().filter(|(k, _)| !secret_header(k)).cloned().collect();
        let scan = if let Scan::Done(report) = &self.scan { Some(report.clone()) } else { None };
        Stored { download: self.download.clone(), headers, downloaded, total, sha256: self.sha256.clone(), scan, replaces: self.replaces }
    }
}

fn secret_header(name: &str) -> bool {
    SECRET_HEADERS.iter().any(|secret| name.eq_ignore_ascii_case(secret))
}

#[derive(Serialize, Deserialize)]
pub(super) struct Stored {
    download: Download,
    headers: Vec<(String, String)>,
    downloaded: u64,
    total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    /// Last VirusTotal verdict: the badge survives restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scan: Option<Report>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    replaces: bool,
}

/// The list saved by the previous session. Entries this version cannot read (written by a newer
/// RDM) are left out, not the whole list; an unreadable file is kept aside (`.bad`) rather than
/// overwritten by the next save.
pub(super) fn load_entries() -> Vec<Entry> {
    load_json::<Vec<serde_json::Value>>(STORE).into_iter().filter_map(restore).collect()
}

/// One saved download back in the list; `None` for one this version cannot read, or whose file is
/// not a file path RDM would have chosen (a hand-edited list: removing it would delete that path).
fn restore(saved: serde_json::Value) -> Option<Entry> {
    let mut s = serde_json::from_value::<Stored>(saved).ok()?;
    let target = &s.download.target;
    if !target.is_absolute() || target.file_name().is_none() {
        return None;
    }
    let digest = |h: &str| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit());
    s.sha256 = s.sha256.filter(|h| digest(h));
    // A report's fingerprint makes the address of its page (see `Report::link`).
    s.scan = s.scan.filter(|r| digest(&r.sha256));
    // Interrupted by exit (quit, shutdown, crash): downloads pick up where they were, from their
    // resume point; a recording cannot continue without its browser tab.
    if *s.download.status() == Status::Running {
        let _ = if s.download.is_recording() {
            s.download.fail(Failure::RecordingInterrupted)
        } else {
            s.download.retry_later()
        };
    }
    let mut entry = Entry::new(s.download, s.headers, s.downloaded, s.total);
    entry.sha256 = s.sha256;
    entry.scan = s.scan.map_or(Scan::None, Scan::Done);
    entry.replaces = s.replaces;
    Some(entry)
}

/// By their bytes: a cookie may hold UTF-8, as browsers send it. Taken for plain ASCII, the whole
/// header would be dropped, and a protected link answer 403.
pub(super) fn to_header_map(pairs: &[(String, String)]) -> HeaderMap {
    pairs
        .iter()
        .filter_map(|(k, v)| Some((HeaderName::from_bytes(k.as_bytes()).ok()?, HeaderValue::from_bytes(v.as_bytes()).ok()?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(target: PathBuf, sha256: Option<&str>) -> serde_json::Value {
        let download = Download::new("https://x.io/a.zip".parse().unwrap(), target, 8);
        let stored = Stored { download, headers: Vec::new(), downloaded: 0, total: 0, sha256: sha256.map(str::to_owned), scan: None, replaces: false };
        serde_json::to_value(stored).unwrap()
    }

    #[test]
    fn a_download_saved_with_a_relative_or_folder_path_is_left_out() {
        let folder_only = std::env::temp_dir().ancestors().last().unwrap().to_path_buf();

        assert!(restore(saved(PathBuf::from("a.zip"), None)).is_none(), "relative: it would follow the working folder");
        assert!(restore(saved(folder_only, None)).is_none(), "a root: removing it would delete that path");
        assert!(restore(saved(std::env::temp_dir().join("a.zip"), None)).is_some());
    }

    #[test]
    fn a_cookie_holding_utf8_is_sent_as_the_browser_sent_it() {
        let pairs = [("cookie".to_owned(), "name=Zoé".to_owned()), ("bad\nname".to_owned(), "x".to_owned())];

        let headers = to_header_map(&pairs);

        assert_eq!(headers.get("cookie").unwrap().as_bytes(), "name=Zoé".as_bytes());
        assert_eq!(headers.len(), 1, "what no server could read is left out");
    }

    #[test]
    fn a_finished_download_keeps_no_session_secret() {
        let download = Download::new("https://x.io/a.zip".parse().unwrap(), std::env::temp_dir().join("a.zip"), 8);
        let headers = vec![("cookie".to_owned(), "sid=1".to_owned()), ("referer".to_owned(), "https://x.io/".to_owned())];
        let mut entry = Entry::new(download, headers, 0, 0);

        entry.forget_secrets();

        assert_eq!(entry.headers, [("referer".to_owned(), "https://x.io/".to_owned())]);
    }

    #[test]
    fn changing_origin_filters_each_credential_independently() {
        let moved = |to: &str, cookie: bool| {
            let download = Download::new("https://www.example.com/a.zip".parse().unwrap(), std::env::temp_dir().join("a.zip"), 8);
            let mut headers = vec![("authorization".to_owned(), "Bearer secret".to_owned()), ("referer".to_owned(), "https://www.example.com/".to_owned())];
            if cookie {
                headers.push(("cookie".to_owned(), "sid=1".to_owned()));
            }
            let mut entry = Entry::new(download, headers, 0, 0);
            entry.moved_to(to.parse().unwrap());
            to_header_map(&entry.headers)
        };
        assert!(moved("https://www.example.com/b.zip", false).contains_key("authorization"), "a same-origin login survives without a cookie");
        for to in ["https://cdn.example.com/a.zip", "https://www.example.com:8443/a.zip", "http://www.example.com/a.zip"] {
            assert!(!moved(to, true).contains_key("authorization"), "a login must not follow to {to}");
        }
    }

    #[test]
    fn secrets_are_neither_stored_nor_retained_regardless_of_header_case() {
        let download = Download::new("https://example.com/a.zip".parse().unwrap(), std::env::temp_dir().join("a.zip"), 8);
        let headers = ["Cookie", "Authorization", "Proxy-Authorization", "referer"].map(|k| (k.to_owned(), "secret".to_owned())).to_vec();
        let mut entry = Entry::new(download, headers, 0, 0);
        assert_eq!(entry.stored().headers, [("referer".to_owned(), "secret".to_owned())]);
        entry.forget_secrets();
        assert_eq!(entry.headers, [("referer".to_owned(), "secret".to_owned())]);
    }

    #[test]
    fn a_malformed_fingerprint_is_dropped_before_it_reaches_a_url() {
        let target = std::env::temp_dir().join("a.zip");
        let sha256 = "ab".repeat(32);

        let kept = restore(saved(target.clone(), Some(&sha256))).unwrap();
        let dropped = restore(saved(target.clone(), Some("../users/me?x=1"))).unwrap();

        assert_eq!(kept.sha256, Some(sha256.clone()));
        assert_eq!(dropped.sha256, None);

        let with_report = |fingerprint: &str| {
            let report = Report { malicious: 0, suspicious: 0, harmless: 1, undetected: 0, unavailable: 0, detections: Vec::new(), sha256: fingerprint.to_owned() };
            let mut value = saved(target.clone(), None);
            value["scan"] = serde_json::to_value(report).unwrap();
            restore(value).unwrap().scan
        };
        assert!(matches!(with_report(&sha256), Scan::Done(_)));
        assert!(matches!(with_report("../../evil?x="), Scan::None), "it would make the report's address");
    }

    /// Regression: "Change the link" kept the old link's cookies and certificate exemption, and sent
    /// them to whatever site the new link was on.
    #[test]
    fn a_new_link_keeps_what_the_old_one_was_granted_on_its_own_site_only() {
        let cookie = |entry: &Entry| entry.headers.iter().any(|(k, _)| k == "cookie");
        let moved = |to: &str| {
            let mut download = Download::new("https://www.example.com/a.zip".parse().unwrap(), std::env::temp_dir().join("a.zip"), 8);
            download.insecure = true;
            let headers = vec![("cookie".to_owned(), "sid=1".to_owned()), ("referer".to_owned(), "https://www.example.com/".to_owned())];
            let mut entry = Entry::new(download, headers, 0, 0);
            entry.moved_to(to.parse().unwrap());
            entry
        };

        let same_host = moved("https://www.example.com/b.zip");
        assert!(cookie(&same_host) && same_host.download.insecure);
        assert_eq!(same_host.download.url.as_str(), "https://www.example.com/b.zip");

        let sibling = moved("https://cdn.example.com/a.zip");
        assert!(!cookie(&sibling), "raw cookies have no domain scope proving they can be shared");
        assert!(!sibling.download.insecure, "an exemption is for one host");

        let elsewhere = moved("https://mirror.example.net/a.zip");
        assert!(!cookie(&elsewhere) && !elsewhere.download.insecure);
        assert_eq!(elsewhere.headers.len(), 1, "only the secrets go");

        assert!(!cookie(&moved("http://www.example.com/a.zip")), "never in clear");
    }

    /// Regression: an automatic update quit RDM while a finished file was being verified or
    /// analysed: the verdict, kept in memory only, was never given.
    #[test]
    fn an_automatic_update_waits_for_what_quitting_would_interrupt() {
        let entry = |status: fn(&mut Download)| {
            let mut download = Download::new("https://x.io/a.zip".parse().unwrap(), std::env::temp_dir().join("a.zip"), 8);
            status(&mut download);
            Entry::new(download, Vec::new(), 0, 0)
        };
        let completed = |d: &mut Download| {
            d.start().unwrap();
            d.complete().unwrap();
        };
        assert!(entry(|_| {}).keeps_rdm_open(), "queued");
        assert!(entry(|d| d.start().unwrap()).keeps_rdm_open(), "running");
        assert!(!entry(|d| d.pause().unwrap()).keeps_rdm_open(), "paused: resumed after the update");
        assert!(!entry(completed).keeps_rdm_open());
        let mut verifying = entry(completed);
        verifying.verify = Verify::Running;
        assert!(verifying.keeps_rdm_open());
        let mut scanning = entry(completed);
        scanning.scan = Scan::Running(Stage::Uploading(0.5));
        assert!(scanning.keeps_rdm_open());
    }
}
