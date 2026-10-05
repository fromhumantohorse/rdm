//! Online services: update checks, VirusTotal scans and the YouTube module.

use super::*;

impl Manager {
    /// Client for VirusTotal and GitHub (not the download engine's: see `virustotal::client`),
    /// through the proxy of the settings like downloads; rebuilt when that changes. Refused for an
    /// unusable proxy address: never around the proxy the user counts on.
    pub(super) fn web(&self) -> Result<reqwest::Client, RouteError> {
        let route = self.route(false);
        let mut web = lock(&self.web);
        if let Some((built_for, client)) = web.as_ref()
            && *built_for == route
        {
            return Ok(client.clone());
        }
        let client = virustotal::client(&route).map_err(|_| RouteError::InvalidProxy)?;
        *web = Some((route, client.clone()));
        Ok(client)
    }

    pub fn update_state(&self) -> update::State {
        lock(&self.update).clone()
    }

    pub(super) fn set_update(&self, state: update::State) {
        *lock(&self.update) = state;
        self.repaint();
    }

    /// At start (after a few seconds) and once a day, if enabled in the settings; sooner when the
    /// release was seen before its package was attached. With automatic updates on, a release RDM
    /// installs without asking anything is installed as soon as nothing is downloading.
    pub(super) fn spawn_update_checks(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.rt.spawn(async move {
            tokio::time::sleep(Duration::from_secs(8)).await;
            loop {
                let Some(this) = weak.upgrade() else { return };
                if this.with_settings(|s| s.check_updates) {
                    this.check_updates(false);
                }
                drop(this);
                // The check runs in the background: give it time to land before looking at it.
                tokio::time::sleep(Duration::from_secs(60)).await;
                let incomplete = weak.upgrade().is_some_and(|this| {
                    update::method() != update::Method::Manual && this.update_state().release().is_some_and(|r| r.package.is_none())
                });
                let next = tokio::time::Instant::now() + if incomplete { Duration::from_secs(15 * 60) } else { UPDATE_EVERY };
                // Meanwhile, once a minute: install when idle, if allowed.
                while tokio::time::Instant::now() < next {
                    let Some(this) = weak.upgrade() else { return };
                    this.auto_install();
                    drop(this);
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            }
        });
    }

    /// Automatic updates: installs the release on offer when RDM can do it without any prompt
    /// (Windows installer, Linux binary in the user's folders) and nothing is downloading.
    pub(super) fn auto_install(self: &Arc<Self>) {
        let silent = matches!(update::method(), update::Method::Msi | update::Method::Binary);
        let ready = matches!(self.update_state(), update::State::Available(ref r) if update::installs_itself(r));
        if !silent || !ready || !self.with_settings(|s| s.auto_update) || self.is_closing() {
            return;
        }
        let stats = self.stats();
        let recording = lock(&self.entries).iter().any(|e| e.download.is_recording() && *e.download.status() == Status::Running);
        if stats.running == 0 && stats.queued == 0 && !recording {
            self.install_update();
        }
    }

    /// Asks GitHub for a newer release. `manual`: the user clicked, so "up to date" and errors show.
    pub fn check_updates(self: &Arc<Self>, manual: bool) {
        let current = self.update_state();
        // A release first seen without its package (published a minute before the build finished
        // uploading it) is looked at again.
        if current.busy() || (!manual && current.release().is_some_and(|r| r.package.is_some())) {
            return;
        }
        if manual {
            self.set_update(update::State::Checking);
        }
        let this = self.clone();
        self.rt.spawn(async move {
            let found = match this.web() {
                Ok(client) => update::check(&client).await.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let state = match found {
                Ok(Some(release)) => update::State::Available(release),
                Ok(None) if manual => update::State::UpToDate,
                Ok(None) => update::State::Idle,
                Err(reason) if manual => update::State::Failed(reason),
                Err(_) => return,
            };
            this.set_update(state);
        });
    }

    /// Downloads and checks the package (signature included), then installs it without any window:
    /// on Windows the installation assistant takes over once RDM has quit; on Linux the package is
    /// installed first, then RDM quits and starts again. `false` when RDM cannot install this
    /// release itself (the UI then opens the release page).
    pub fn install_update(self: &Arc<Self>) -> bool {
        let Some(release) = self.update_state().release().cloned() else { return false };
        let Some(package) = release.package.clone().filter(|_| update::installs_itself(&release)) else { return false };
        self.set_update(update::State::Downloading(0.0));
        let this = self.clone();
        self.rt.spawn(async move {
            // The update stays on offer: the card lets the user try again.
            let fail = |reason: String| this.set_update(update::State::InstallFailed(release.clone(), reason));
            let client = match this.web() {
                Ok(client) => client,
                Err(e) => return fail(e.to_string()),
            };
            let progress = {
                let this = this.clone();
                move |f: f32| {
                    let mut state = lock(&this.update);
                    // One repaint per percent is plenty.
                    if matches!(*state, update::State::Downloading(old) if (f - old).abs() < 0.01 && f < 1.0) {
                        return;
                    }
                    *state = update::State::Downloading(f);
                    drop(state);
                    this.repaint();
                }
            };
            let file = match update::download(&client, &release.version, &package, progress).await {
                Ok(file) => file,
                Err(e) => return fail(e.to_string()),
            };
            this.set_update(update::State::Installing);
            if update::method() == update::Method::Msi {
                let started = tokio::task::spawn_blocking({
                    let file = file.clone();
                    move || update::start_installation(&file)
                })
                .await
                .unwrap_or_else(|e| Err(update::UpdateError::CannotStart(std::io::Error::other(e))));
                match started {
                    Ok(()) => this.request_quit(),
                    Err(e) => fail(e.to_string()),
                }
                return;
            }
            let result = update::install_linux(file.clone()).await;
            let _ = tokio::fs::remove_file(&file.path).await;
            match result {
                Ok(()) => {
                    this.restart_after_exit.store(true, Release);
                    this.request_quit();
                }
                Err(e) => fail(e.to_string()),
            }
        });
        true
    }

    /// Installs the YouTube module (see `ytdlp`) the way downloads reach the Internet.
    pub fn install_youtube_module(&self) {
        crate::ytdlp::install(&self.rt, self.route(false));
    }

    /// The links of YouTube video `id`, read by the YouTube module on the route their download
    /// takes: YouTube binds them to the address that reads them.
    pub async fn youtube_links(&self, id: &str) -> Result<crate::ytdlp::Formats, crate::ytdlp::ModuleError> {
        crate::ytdlp::extract(id, &self.route(false)).await
    }

    /// Has VirusTotal analyse a finished file (looked up by hash first, uploaded only if unknown),
    /// entirely in the background; the verdict lands in the entry and in a desktop notification.
    pub fn scan_virustotal(self: &Arc<Self>, id: DownloadId) -> Result<(), ScanRefused> {
        let key = lock(&self.secrets).virustotal_key.clone();
        if key.is_empty() {
            return Err(ScanRefused::NoKey);
        }
        let job = self.update_quiet(id, |e| {
            if !e.scannable() || matches!(e.scan, Scan::Running(_)) {
                return None;
            }
            e.scan = Scan::Running(Stage::Queued);
            Some((e.download.target.clone(), e.sha256.clone(), e.name.clone()))
        });
        let Some(Some((path, known_hash, name))) = job else { return Err(ScanRefused::NotEligible) };

        let this = self.clone();
        let gate = self.scan_gate.clone();
        self.rt.spawn(async move {
            let _turn = gate.acquire_owned().await;
            let stage: virustotal::OnStage = {
                let this = this.clone();
                Arc::new(move |stage| {
                    this.update_quiet(id, |e| e.scan = Scan::Running(stage));
                })
            };
            let result = async {
                let sha256 = match known_hash {
                    Some(hash) => hash,
                    None => {
                        stage(Stage::Hashing);
                        let file = path.clone();
                        let hash = tokio::task::spawn_blocking(move || checksum::digest(&file, checksum::Algo::Sha256))
                            .await
                            .map_err(|_| virustotal::Error::Io)?
                            .map_err(|_| virustotal::Error::Io)?;
                        this.update(id, |e| e.sha256 = Some(hash.clone()));
                        hash
                    }
                };
                let client = this.web().map_err(|_| virustotal::Error::Network)?;
                virustotal::scan(&client, &key, &path, &sha256, stage).await
            }
            .await;
            notify::virustotal(&name, result.as_ref());
            this.update(id, |e| {
                e.scan = match result {
                    Ok(report) => Scan::Done(report),
                    Err(err) => Scan::Failed(err),
                };
            });
        });
        Ok(())
    }
}
