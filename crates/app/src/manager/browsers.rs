//! Browsers: extension installs, Firefox pairing and check-ins.

use super::*;

/// Firefox gives each install of an extension a random origin (`moz-extension://<uuid>`), which
/// cannot be pinned like Chrome's: the native connector pairs it (the browser vouches for the
/// extension), or, without the connector, the user approves it once in the RDM window.
#[derive(Default)]
pub(super) struct FirefoxPairing {
    /// Firefox, Waterfox, LibreWolf… each profile has its own origin; the latest few are kept.
    pub(super) paired: VecDeque<String>,
    pub(super) pending: Option<String>,
    /// Refused this session: never asked again until RDM restarts.
    pub(super) refused: HashSet<String>,
    /// "Later" (✕): the extension's periodic check-ins do not bring the question back before this.
    pub(super) snoozed_until: Option<Instant>,
}

/// How long "later" on the Firefox question lasts, unless the user clicks the extension's button.
const FIREFOX_SNOOZE: Duration = Duration::from_secs(30 * 60);

const FIREFOX_FILE: &str = "firefox.json";
const MAX_PAIRED: usize = 8;
/// Browsers the extension may report from (see `Manager::browser_seen`).
const MAX_BROWSERS: usize = 32;

pub(super) const BROWSERS_FILE: &str = "browsers.json";
/// Extensions to remove (see `Manager::remove_extension`).
pub(super) const UNINSTALLS_FILE: &str = "uninstalls.json";
const UNINSTALL_TTL_SECS: u64 = 24 * 3600;
/// Browsers added by hand (see `Manager::add_browser`).
pub(super) const CUSTOM_BROWSERS_FILE: &str = "custom_browsers.json";
/// How long after RDM opened a browser to install the extension its tab is still taken as RDM's.
const INSTALL_TAB_TTL: Duration = Duration::from_secs(15 * 60);

/// How often the running browsers are looked at (see `Manager::watch_browsers`).
const WATCH_EVERY: Duration = Duration::from_secs(5);
/// A browser that just started checks in within seconds: past this, it has no extension.
const STARTED_GRACE: Duration = Duration::from_secs(30);
/// A browser already running when RDM starts checks in at its next heartbeat (every 5 minutes); so
/// does, at the latest, one that had the extension: a slow start is no reason to offer it again.
const RUNNING_GRACE: Duration = Duration::from_secs(6 * 60);
/// What Chromium-based browsers without a name of their own report (see `BROWSER` in
/// `extension/background.js`): Vivaldi, for one.
const NAMELESS_CHROMIUM: &str = "chromium";

/// A browser the user has open, as `Manager::watch_browsers` follows it.
struct OpenBrowser {
    /// Since when (Unix time) its extension is expected to have checked in.
    since: u64,
    /// When to judge whether it did; `None` once judged.
    judge_at: Option<Instant>,
}

/// Installing the extension into one browser, as the extension window shows it.
#[derive(Debug, Clone)]
pub enum Install {
    Working,
    Done(Installed),
    Failed(String),
}

#[derive(Debug, Clone, Default)]
pub struct Installed {
    /// The unpacked extension (Chromium-based browsers: "load unpacked" from here).
    pub folder: PathBuf,
    /// The browser was opened on its extensions page (or on the package to confirm).
    pub launched: bool,
    /// The browser itself now asks the user to add the extension: nothing to explain.
    pub asks: bool,
    /// Firefox: the signed package is installing for good (Firefox asks to confirm).
    pub signed: bool,
    /// Chromium: the extension's page in the Chrome Web Store, which lists it.
    pub store: bool,
    /// Firefox: the unsigned package, for the editions that accept one.
    pub xpi: Option<PathBuf>,
}

impl FirefoxPairing {
    /// `firefox.json`: a list of origins (RDM 0.2 wrote a single one).
    pub(super) fn load() -> VecDeque<String> {
        let bytes = crate::settings::read_config(&crate::settings::config_file(FIREFOX_FILE)).unwrap_or_default();
        let origins = serde_json::from_slice::<VecDeque<String>>(&bytes)
            .or_else(|_| serde_json::from_slice::<String>(&bytes).map(|o| VecDeque::from([o])))
            .unwrap_or_default();
        origins.into_iter().filter(|o| is_firefox_origin(o)).take(MAX_PAIRED).collect()
    }

    fn pair(&mut self, origin: String) -> std::io::Result<()> {
        self.refused.remove(&origin);
        if self.pending.as_ref() == Some(&origin) {
            self.pending = None;
        }
        if self.paired.contains(&origin) {
            return Ok(());
        }
        self.paired.push_back(origin);
        while self.paired.len() > MAX_PAIRED {
            self.paired.pop_front();
        }
        save_json(FIREFOX_FILE, &self.paired)
    }
}

/// An origin a browser drew at random for an extension: Firefox per install, Safari per launch.
fn is_firefox_origin(origin: &str) -> bool {
    ["moz-extension://", "safari-web-extension://"].iter().filter_map(|scheme| origin.strip_prefix(scheme)).any(|id| {
        id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
    })
}

/// Whether the extension of browser `key` was heard from since `since`: under its own name, or
/// under another of its family (`same_family`) that is not running: a browser reporting a name not
/// its own — Vivaldi gives none, and is `chromium` — must not be taken as lacking the extension.
fn heard_since(seen: &BTreeMap<String, u64>, key: &str, since: u64, running: &HashSet<&str>, same_family: impl Fn(&str) -> bool) -> bool {
    seen.iter().any(|(other, &at)| at >= since && (other == key || (!running.contains(other.as_str()) && same_family(other))))
}

/// Whether the Chrome Web Store has the extension to give: it answers the browsers' own question
/// with the package's address, or an error for an extension it does not list (yet).
async fn in_chrome_store(client: &reqwest::Client) -> bool {
    let id = crate::bridge::STORE_EXTENSION_ORIGIN.trim_start_matches("chrome-extension://");
    let url = format!("https://clients2.google.com/service/update2/crx?response=updatecheck&acceptformat=crx3&prodversion=140.0&x=id%3D{id}%26uc");
    match client.get(url).timeout(Duration::from_secs(8)).send().await {
        Ok(answer) => answer.text().await.is_ok_and(|text| store_gives_a_package(&text)),
        Err(_) => false,
    }
}

fn store_gives_a_package(answer: &str) -> bool {
    answer.contains(" codebase=\"https://")
}

impl Manager {
    /// Whether this Firefox extension origin may use the bridge; an unknown one is put up for the
    /// user's approval (the window comes to the front) and refused meanwhile.
    pub fn firefox_allowed(&self, origin: &str) -> bool {
        if !is_firefox_origin(origin) {
            return false;
        }
        let mut ff = lock(&self.firefox);
        if ff.paired.iter().any(|o| o == origin) {
            return true;
        }
        let snoozed = ff.snoozed_until.is_some_and(|t| Instant::now() < t);
        if ff.pending.is_none() && !ff.refused.contains(origin) && !snoozed {
            ff.pending = Some(origin.to_owned());
            drop(ff);
            self.show();
            self.repaint();
        }
        false
    }

    /// The user clicked the extension's button: a question put off with "later" comes back now.
    pub fn firefox_wake(&self) {
        lock(&self.firefox).snoozed_until = None;
    }

    /// The Firefox origin waiting for the user's approval, if any.
    pub fn firefox_pending(&self) -> Option<String> {
        lock(&self.firefox).pending.clone()
    }

    /// `None`: dismissed without answering — asked again at the extension's next request.
    pub fn answer_firefox(&self, allow: Option<bool>) {
        let mut ff = lock(&self.firefox);
        let Some(origin) = ff.pending.take() else { return };
        match allow {
            Some(true) => {
                let saved = ff.pair(origin);
                drop(ff);
                self.saved(saved);
            }
            Some(false) => {
                ff.refused.insert(origin);
            }
            None => ff.snoozed_until = Some(Instant::now() + FIREFOX_SNOOZE),
        }
    }

    /// The native connector vouches for this Firefox extension origin (the browser started the
    /// connector for the RDM extension only): paired without asking.
    pub fn pair_firefox(&self, origin: &str) -> bool {
        if !is_firefox_origin(origin) {
            return false;
        }
        let saved = lock(&self.firefox).pair(origin.to_owned());
        self.saved(saved);
        self.repaint();
        true
    }

    /// What Safari's extension shows from its next launch on, when its origin will have changed:
    /// given once approved, kept with the passwords. Made when first asked for.
    pub fn safari_pair(&self) -> String {
        let mut secrets = lock(&self.secrets);
        if !secrets.safari_pair.is_empty() {
            return secrets.safari_pair.clone();
        }
        secrets.safari_pair = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let (pair, saved) = (secrets.safari_pair.clone(), secrets.save());
        drop(secrets);
        self.saved(saved);
        pair
    }

    /// Whether `given` is what `safari_pair` gave.
    pub fn safari_pair_matches(&self, given: &[u8]) -> bool {
        let secrets = lock(&self.secrets);
        !secrets.safari_pair.is_empty() && crate::local::token_matches(&secrets.safari_pair, Some(given))
    }

    /// The extension reported which browser it runs in (`x-rdm-browser`).
    pub fn browser_seen(&self, key: &str) {
        // A computer has a handful of browsers: past this, a caller making names up is ignored
        // (each browser met is kept for the whole session).
        let room = {
            let seen = lock(&self.browsers);
            seen.contains_key(key) || seen.len() < MAX_BROWSERS
        };
        if !room {
            return;
        }
        let Some(browser) = Browser::from_key(key) else { return };
        if self.uninstall_requested(key) {
            return; // on its way out: not "installed" again
        }
        let now = unix_now();
        let mut seen = lock(&self.browsers);
        let before = seen.insert(browser.key().to_owned(), now);
        // Written when news, not on every request (recordings post chunks many times a second).
        if before.is_none_or(|t| now.saturating_sub(t) > 600) {
            let copy = seen.clone();
            drop(seen);
            self.saved(save_json(BROWSERS_FILE, &copy));
            self.repaint();
        }
    }

    /// When the extension was last heard from, per browser (Unix time).
    pub fn browser_last_seen(&self, browser: Browser) -> Option<u64> {
        lock(&self.browsers).get(browser.key()).copied()
    }

    /// "Remove the extension": RDM forgets it in `browser` at once, and the extension, if still
    /// there, uninstalls itself at its next check-in (at the browser's start, then every few minutes).
    pub fn remove_extension(&self, browser: Browser) {
        let key = browser.key().to_owned();
        let seen = {
            let mut seen = lock(&self.browsers);
            seen.remove(&key);
            seen.clone()
        };
        self.saved(save_json(BROWSERS_FILE, &seen));
        let pending = {
            let mut pending = lock(&self.uninstalls);
            pending.insert(key, unix_now());
            pending.clone()
        };
        self.saved(save_json(UNINSTALLS_FILE, &pending));
        lock(&self.installs).remove(&browser);
        self.repaint();
    }

    /// Whether the extension in the browser named `key` must uninstall itself.
    pub fn uninstall_requested(&self, key: &str) -> bool {
        lock(&self.uninstalls).get(key).is_some_and(|&t| unix_now().saturating_sub(t) < UNINSTALL_TTL_SECS)
    }

    /// The request is over: the extension is uninstalling itself, or the user installs it again.
    pub(super) fn forget_uninstall(&self, key: &str) {
        let pending = {
            let mut pending = lock(&self.uninstalls);
            if pending.remove(key).is_none() {
                return;
            }
            pending.clone()
        };
        self.saved(save_json(UNINSTALLS_FILE, &pending));
    }

    /// The extension of the browser named `key` got the request and uninstalls itself now.
    pub fn extension_uninstalled(&self, key: &str) {
        self.forget_uninstall(key);
        self.repaint();
    }

    /// Every browser the extension was heard from, with when (Unix time).
    pub fn browsers_seen(&self) -> Vec<(Browser, u64)> {
        lock(&self.browsers).iter().filter_map(|(key, &t)| Some((Browser::from_key(key)?, t))).collect()
    }

    /// The browsers of this computer (Windows' list, the well-known ones, those added by hand),
    /// then those the extension was heard from that are not among them (a portable browser).
    pub fn browsers(&self) -> Vec<Browser> {
        let mut list = extension::installed(&lock(&self.custom_browsers));
        for (b, _) in self.browsers_seen() {
            if !list.contains(&b) {
                list.push(b);
            }
        }
        list
    }

    /// "Add a browser…": the browser at `exe`, remembered. `None`: not a browser the extension
    /// can run in (neither Chromium- nor Firefox-based).
    pub fn add_browser(&self, exe: PathBuf) -> Option<Browser> {
        let browser = Browser::at(&exe)?;
        let list = {
            let mut list = lock(&self.custom_browsers);
            if !list.contains(&exe) {
                list.push(exe);
            }
            list.clone()
        };
        self.saved(save_json(CUSTOM_BROWSERS_FILE, &list));
        Some(browser)
    }

    /// Whether an installation is being prepared (the window shows a spinner).
    pub fn installing(&self) -> bool {
        lock(&self.installs).values().any(|i| matches!(i, Install::Working))
    }

    /// The extension just installed in the browser named `key`: whether RDM opened that browser
    /// for it a moment ago (then the tab it opened can go). Answered once.
    pub fn installed_by_rdm(&self, key: &str) -> bool {
        lock(&self.opened_to_install).remove(key).is_some_and(|at| at.elapsed() < INSTALL_TAB_TTL)
    }

    pub fn install_state(&self, browser: Browser) -> Option<Install> {
        lock(&self.installs).get(&browser).cloned()
    }

    /// Prepares the extension for `browser` and opens the browser where the user confirms it.
    pub fn install_extension(self: &Arc<Self>, browser: Browser) {
        self.start_install(browser, false);
    }

    /// `offered`: RDM's own idea, not the user's click. Where the browser itself asks (its "Add"
    /// prompt) nothing more is needed; otherwise the window shows the steps.
    fn start_install(self: &Arc<Self>, browser: Browser, offered: bool) {
        self.forget_uninstall(browser.key());
        if matches!(lock(&self.installs).insert(browser, Install::Working), Some(Install::Working)) {
            return; // already on it
        }
        self.repaint();
        let this = self.clone();
        self.rt.spawn(async move {
            let state = match this.install(browser).await {
                Ok(done) => Install::Done(done),
                Err(e) => Install::Failed(trf!("impossible d'écrire l'extension : {e}", "cannot write the extension: {e}", e = e)),
            };
            if matches!(&state, Install::Done(done) if done.launched) {
                lock(&this.opened_to_install).insert(browser.key().to_owned(), Instant::now());
            }
            let asks = matches!(&state, Install::Done(done) if done.asks);
            lock(&this.installs).insert(browser, state);
            if offered && !asks {
                *lock(&this.extension_offer) = Some(browser);
                this.show();
            }
            this.repaint();
        });
    }

    pub(super) async fn install(&self, browser: Browser) -> std::io::Result<Installed> {
        let flavour = browser.flavour();
        if flavour == Flavour::Safari {
            let launched = tokio::task::spawn_blocking(extension::open_safari_settings).await.unwrap_or(false);
            return Ok(Installed { launched, ..Installed::default() });
        }
        let exe = tokio::task::spawn_blocking(move || browser.find()).await?;
        // The connector too (a browser installed since RDM started has not got it yet).
        let _ = tokio::task::spawn_blocking(crate::native::register).await;
        let folder = tokio::task::spawn_blocking(move || extension::write(flavour)).await??;
        let mut done = Installed { folder, ..Installed::default() };
        let open = |target: &str| exe.as_deref().is_some_and(|exe| extension::launch(exe, target).is_ok());
        if flavour == Flavour::Chromium
            && let Ok(client) = self.web()
            && in_chrome_store(&client).await
        {
            done.store = true;
            done.launched = open(&extension::store_page());
            done.asks = done.launched;
            return Ok(done);
        }
        if flavour == Flavour::Firefox {
            done.xpi = tokio::task::spawn_blocking(extension::write_xpi).await?.ok();
            let signed = extension::base().join("rdm-firefox-signed.xpi");
            if let Ok(client) = self.web()
                && update::signed_firefox_xpi(&client, &signed).await.unwrap_or(false)
            {
                done.signed = true;
                done.launched = open(&signed.to_string_lossy());
                done.asks = done.launched;
                return Ok(done);
            }
            // Waterfox can install the unsigned package for good (signature check off: see the steps).
            if browser.key() == "waterfox"
                && let Some(xpi) = &done.xpi
            {
                done.launched = open(&xpi.to_string_lossy());
                done.asks = done.launched;
                return Ok(done);
            }
        }
        done.launched = open(browser.extensions_page());
        Ok(done)
    }

    /// Like IDM, and at every opening: a browser the user opens without the extension gets it offered
    /// (see `start_install`), while the "Offer the extension" setting is on. So does the default
    /// browser, when RDM itself is opened by the user (`interactive`), not started at login. The
    /// thread holds the manager only for each look, and ends with it.
    pub fn watch_browsers(self: &Arc<Self>, interactive: bool) {
        let this = Arc::downgrade(self);
        std::thread::spawn(move || {
            if interactive
                && let Some(manager) = this.upgrade()
                && manager.with_settings(|s| s.offer_extension)
            {
                manager.offer_in_default_browser();
            }
            let mut open: HashMap<&'static str, OpenBrowser> = HashMap::new();
            let mut first_look = true;
            loop {
                let Some(manager) = this.upgrade().filter(|m| !m.is_closing()) else { return };
                if manager.with_settings(|s| s.offer_extension) {
                    manager.look_at_browsers(&mut open, first_look);
                    first_look = false;
                }
                drop(manager);
                std::thread::sleep(WATCH_EVERY);
            }
        });
    }

    fn offer_in_default_browser(self: &Arc<Self>) {
        let Some(browser) = extension::default_browser() else { return };
        let seen = |key: &str| lock(&self.browsers).contains_key(key);
        let nameless = browser.flavour() == Flavour::Chromium && seen(NAMELESS_CHROMIUM);
        if !seen(browser.key()) && !nameless && self.may_offer(browser) {
            self.start_install(browser, true);
        }
    }

    /// One look at what runs: a browser just opened is given the time to check in, then judged once,
    /// until it closes: its next opening starts all over.
    fn look_at_browsers(self: &Arc<Self>, open: &mut HashMap<&'static str, OpenBrowser>, first_look: bool) {
        let running = extension::running();
        open.retain(|key, _| {
            let still = running.contains(key);
            if !still {
                lock(&self.opened_to_install).remove(*key);
            }
            still
        });
        let now = Instant::now();
        for &key in &running {
            let known = lock(&self.browsers).contains_key(key);
            let state = open.entry(key).or_insert_with(|| OpenBrowser {
                // What it said while it opened, before this look saw it, counts.
                since: unix_now().saturating_sub(WATCH_EVERY.as_secs() + 1),
                judge_at: Some(now + if first_look || known { RUNNING_GRACE } else { STARTED_GRACE }),
            });
            if !state.judge_at.is_some_and(|at| now >= at) {
                continue;
            }
            state.judge_at = None;
            let flavour_of = |key: &str| Browser::from_key(key).map(Browser::flavour);
            let heard = heard_since(&lock(&self.browsers), key, state.since, &running, |other| flavour_of(other) == flavour_of(key));
            if heard {
                continue;
            }
            if let Some(browser) = self.browsers().into_iter().find(|b| b.key() == key).filter(|&b| self.may_offer(b)) {
                self.start_install(browser, true);
            }
        }
    }

    /// Not when the user asked RDM to remove it, when RDM opened that browser for it a moment ago, nor
    /// for a Firefox-family browser whose extension is there, waiting for the user's word in RDM.
    fn may_offer(&self, browser: Browser) -> bool {
        let key = browser.key();
        let just_opened = lock(&self.opened_to_install).get(key).is_some_and(|at| at.elapsed() < INSTALL_TAB_TTL);
        if self.uninstall_requested(key) || just_opened {
            return false;
        }
        let ff = lock(&self.firefox);
        browser.flavour() == Flavour::Chromium || (ff.pending.is_none() && ff.refused.is_empty() && ff.snoozed_until.is_none_or(|t| Instant::now() >= t))
    }

    /// The browser whose extension offer the window must show, once.
    pub fn take_extension_offer(&self) -> Option<Browser> {
        lock(&self.extension_offer).take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_a_browser_draws_at_random_are_told_apart_from_others() {
        const ID: &str = "0f8e7a1c-3b2d-4e5f-9a8b-7c6d5e4f3a2b";
        assert!(is_firefox_origin(&format!("moz-extension://{ID}")));
        assert!(is_firefox_origin(&format!("safari-web-extension://{}", ID.to_ascii_uppercase())));
        assert!(!is_firefox_origin(&format!("chrome-extension://{ID}")));
        assert!(!is_firefox_origin("moz-extension://short"));
        assert!(!is_firefox_origin(&format!("https://{ID}")));
    }

    #[test]
    fn the_extension_counts_as_heard_since_the_browser_opened_only() {
        let seen = BTreeMap::from([("chrome".to_owned(), 100), ("chromium".to_owned(), 200), ("firefox".to_owned(), 300)]);
        let chromium_family = |key: &str| key != "firefox";
        let running = |keys: &[&'static str]| keys.iter().copied().collect::<HashSet<&str>>();

        assert!(heard_since(&seen, "firefox", 300, &running(&["firefox"]), |k| k == "firefox"), "under its own name");
        assert!(!heard_since(&seen, "firefox", 301, &running(&["firefox"]), |k| k == "firefox"), "before it opened: gone since, perhaps");
        assert!(heard_since(&seen, "vivaldi", 150, &running(&["vivaldi"]), chromium_family), "a nameless one of its family");
        assert!(!heard_since(&seen, "vivaldi", 150, &running(&["vivaldi", "chromium"]), chromium_family), "Chromium runs: that was Chromium itself");
        assert!(!heard_since(&seen, "edge", 250, &running(&["edge"]), chromium_family), "too long ago");
    }

    /// The Chrome Web Store's real answers, for an extension it lists and one it does not.
    #[test]
    fn the_store_says_when_it_lists_the_extension() {
        let listed = r#"<gupdate protocol="2.0"><app appid="aapb" cohort="1::" status="ok"><updatecheck _esbAllowlist="true" codebase="https://clients2.googleusercontent.com/crx/blobs/x.crx" version="1.0"/></app></gupdate>"#;
        let unknown = r#"<gupdate protocol="2.0"><daystart elapsed_days="7250"/><app appid="adoa" status="error-unknownApplication"/></gupdate>"#;

        assert!(store_gives_a_package(listed));
        assert!(!store_gives_a_package(unknown));
    }
}
