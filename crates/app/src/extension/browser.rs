//! The browsers the extension runs in: known by name, listed by Windows, added by the user, or
//! reported by the extension itself; where each one is, and how to open it on a page.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// A browser the extension can run in: a well-known one, one Windows lists as installed, one the
/// user added, or one the extension reported from. Every browser belongs to one of two families
/// (`Flavour`): what works in Chrome works in any Chromium-based browser, what works in Firefox in
/// any of its derivatives, so none has to be listed here to be supported.
#[derive(Debug, Clone, Copy)]
pub struct Browser(&'static Info);

#[derive(Debug)]
struct Info {
    /// What the extension reports about itself (`x-rdm-browser`): see `key_of`.
    key: String,
    name: String,
    flavour: Flavour,
    /// Where it was found (Windows' list of browsers, or picked by the user).
    exe: Option<PathBuf>,
}

/// Chromium-based browsers share one build; Firefox and its derivatives have their own manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flavour {
    Chromium,
    Firefox,
}

/// Browsers known by name: the name shown and the family. Any other browser works the same,
/// named as Windows or the extension names it.
const KNOWN: &[(&str, &str, Flavour)] = &[
    ("firefox", "Firefox", Flavour::Firefox),
    ("waterfox", "Waterfox", Flavour::Firefox),
    ("librewolf", "LibreWolf", Flavour::Firefox),
    ("floorp", "Floorp", Flavour::Firefox),
    ("zen", "Zen Browser", Flavour::Firefox),
    ("mullvad", "Mullvad Browser", Flavour::Firefox),
    ("chrome", "Google Chrome", Flavour::Chromium),
    ("edge", "Microsoft Edge", Flavour::Chromium),
    ("brave", "Brave", Flavour::Chromium),
    ("opera", "Opera", Flavour::Chromium),
    ("vivaldi", "Vivaldi", Flavour::Chromium),
    ("chromium", "Chromium", Flavour::Chromium),
];

/// Every browser met so far, one entry per key: `Browser` stays a cheap copy.
static SEEN: std::sync::Mutex<Vec<&'static Info>> = std::sync::Mutex::new(Vec::new());

/// Words of a browser's name that do not tell it apart: vendors, and generic words (`NOT_A_NAME`
/// in `extension/shared.js`).
const NOT_A_NAME: [&str; 7] = ["mozilla", "google", "microsoft", "browser", "stable", "web", "the"];

/// The key of a browser's name, as the extension computes it too (`browserKey` in
/// `extension/shared.js`, same tests): the first word that is not the vendor's or a generic one,
/// letters only, at most 16. "Google Chrome" is `chrome`, "Mullvad Browser" is `mullvad`.
fn key_of(name: &str) -> String {
    name.split(|c: char| !c.is_ascii_alphabetic())
        .map(str::to_ascii_lowercase)
        .find(|w| !w.is_empty() && !NOT_A_NAME.contains(&w.as_str()))
        .map(|w| w.chars().take(16).collect())
        .unwrap_or_default()
}

pub(crate) fn valid_key(key: &str) -> bool {
    (1..=16).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_lowercase())
}

impl PartialEq for Browser {
    fn eq(&self, other: &Self) -> bool {
        self.0.key == other.0.key
    }
}

impl Eq for Browser {}

impl std::hash::Hash for Browser {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.key.hash(state);
    }
}

impl Browser {
    /// The browser of this key; learnt with `exe` (and the name Windows gives it) when found.
    fn with(key: &str, name: Option<&str>, flavour: Option<Flavour>, exe: Option<PathBuf>) -> Self {
        let mut seen = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let old = seen.iter().position(|i| i.key == key);
        if let Some(i) = old
            && (exe.is_none() || seen[i].exe == exe)
        {
            return Self(seen[i]);
        }
        let old = old.map(|i| seen.remove(i));
        let known = KNOWN.iter().find(|(k, ..)| *k == key);
        // Leaked on purpose: one per browser (and per new executable), a handful in all.
        let info: &'static Info = Box::leak(Box::new(Info {
            key: key.to_owned(),
            // A well-known name first ("Google Chrome", not Windows' "Chrome").
            name: known
                .map(|k| k.1.to_owned())
                .or_else(|| name.map(str::to_owned))
                .or_else(|| old.map(|o| o.name.clone()))
                .unwrap_or_else(|| title(key)),
            flavour: known.map(|k| k.2).or(flavour).or(old.map(|o| o.flavour)).unwrap_or(Flavour::Chromium),
            exe: exe.or_else(|| old.and_then(|o| o.exe.clone())),
        }));
        seen.push(info);
        Self(info)
    }

    /// The browser the extension says it runs in (`x-rdm-browser`); `None` for a malformed key.
    /// One RDM does not know otherwise is taken as Chromium-based until found on this computer.
    pub fn from_key(key: &str) -> Option<Self> {
        valid_key(key).then(|| Self::with(key, None, None, None))
    }

    pub fn name(self) -> &'static str {
        &self.0.name
    }

    pub fn key(self) -> &'static str {
        &self.0.key
    }

    pub fn flavour(self) -> Flavour {
        self.0.flavour
    }

    /// Where extensions are managed (opened for the user, who confirms the installation there).
    /// Chromium-based browsers without a page of their own accept `chrome://extensions`.
    pub fn extensions_page(self) -> &'static str {
        match (self.flavour(), self.key()) {
            (Flavour::Firefox, _) => "about:debugging#/runtime/this-firefox",
            (_, "brave") => "brave://extensions/",
            (_, "opera") => "opera://extensions/",
            (_, "edge") => "edge://extensions/",
            (_, "vivaldi") => "vivaldi://extensions/",
            _ => "chrome://extensions/",
        }
    }

    /// The browser's executable (its application bundle on macOS), if it is installed.
    pub fn find(self) -> Option<PathBuf> {
        self.0.exe.clone().filter(|p| p.exists()).or_else(|| imp::find(self.key()))
    }

    /// The browser at `exe` (added by the user): its family from its files, named after the file.
    /// `None`: not a browser the extension can run in.
    pub fn at(exe: &Path) -> Option<Self> {
        let flavour = flavour_of(exe)?;
        let stem = exe.file_stem()?.to_string_lossy().into_owned();
        let key = key_of(&stem);
        valid_key(&key).then(|| Self::with(&key, Some(&title(&stem)), Some(flavour), Some(exe.to_path_buf())))
    }
}

/// "waterfox" gives "Waterfox".
fn title(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// A browser's family, from the files beside its executable: Firefox and every derivative ship
/// `xul.dll` (`libxul.so`); Chromium-based browsers `.pak` resources (in a version sub-folder on
/// Windows). `None`: not a browser the extension can run in.
#[cfg(not(target_os = "macos"))]
fn flavour_of(exe: &Path) -> Option<Flavour> {
    let dir = exe.parent()?;
    let names = |dir: &Path| -> Vec<String> {
        fs::read_dir(dir).map(|e| e.flatten().map(|e| e.file_name().to_string_lossy().to_ascii_lowercase()).collect()).unwrap_or_default()
    };
    let here = names(dir);
    if here.iter().any(|n| n == "xul.dll" || n == "libxul.so") {
        return Some(Flavour::Firefox);
    }
    let chromium = |list: &[String]| list.iter().any(|n| n.ends_with(".pak") || n == "chrome.dll" || n == "msedge.dll");
    let versions = fs::read_dir(dir).map(|e| e.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect::<Vec<_>>()).unwrap_or_default();
    (chromium(&here) || versions.iter().any(|v| chromium(&names(v)))).then_some(Flavour::Chromium)
}

/// macOS: from the application bundle: Firefox and every derivative hold `XUL`, Chromium-based
/// browsers a `<name> Framework.framework` (Electron applications too, under that name).
#[cfg(target_os = "macos")]
fn flavour_of(app: &Path) -> Option<Flavour> {
    if app.join("Contents/MacOS/XUL").is_file() {
        return Some(Flavour::Firefox);
    }
    let frameworks = fs::read_dir(app.join("Contents/Frameworks")).ok()?;
    frameworks
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .any(|name| name.ends_with(" Framework.framework") && name != "Electron Framework.framework")
        .then_some(Flavour::Chromium)
}

/// The browsers of this computer: those Windows lists as installed (any browser, known here or
/// not), the well-known ones found where they usually are, and those the user added (`custom`).
/// One browser per key, the first found: two of the same key (Firefox and Firefox Developer
/// Edition are both `firefox`) would otherwise take each other's place at every call, each time a
/// new (leaked) `Info`.
pub fn installed(custom: &[PathBuf]) -> Vec<Browser> {
    let mut list: Vec<Browser> = Vec::new();
    let listed = |list: &[Browser], key: &str| list.iter().any(|b| b.key() == key);
    for (name, exe) in imp::registered() {
        let key = key_of(&name);
        if valid_key(&key)
            && !listed(&list, &key)
            && let Some(flavour) = flavour_of(&exe)
        {
            list.push(Browser::with(&key, Some(&name), Some(flavour), Some(exe)));
        }
    }
    for (key, ..) in KNOWN {
        if !listed(&list, key) {
            let b = Browser::with(key, None, None, None);
            if b.find().is_some() {
                list.push(b);
            }
        }
    }
    for exe in custom {
        let key = exe.file_stem().map(|s| key_of(&s.to_string_lossy())).unwrap_or_default();
        if !listed(&list, &key)
            && let Some(b) = Browser::at(exe)
        {
            list.push(b);
        }
    }
    list
}

/// Opens `target` (a page or a package) in the browser whose executable is `exe`.
pub fn launch(exe: &Path, target: &str) -> io::Result<()> {
    use std::process::{Command, Stdio};
    // macOS: a second copy of a browser already running would refuse its profile: the system hands
    // the page to the running one.
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut open = Command::new("/usr/bin/open");
        open.arg("-a").arg(exe);
        open
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = Command::new(exe);
    let mut child = command.arg(target).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    // Reaped off the UI thread: the browser may run for hours (Linux would keep a zombie otherwise).
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(windows)]
mod imp {
    use std::path::PathBuf;

    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    };

    /// Browsers Windows lists as installed (Default apps): name and executable. Every browser
    /// installer registers there, known to RDM or not.
    pub fn registered() -> Vec<(String, PathBuf)> {
        let mut list = Vec::new();
        for (hive, path) in [
            (HKEY_CURRENT_USER, r"SOFTWARE\Clients\StartMenuInternet"),
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\Clients\StartMenuInternet"),
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Clients\StartMenuInternet"),
        ] {
            let Ok(clients) = RegKey::predef(hive).open_subkey(path) else { continue };
            for id in clients.enum_keys().flatten() {
                let Ok(client) = clients.open_subkey(&id) else { continue };
                let name: String = client.get_value("").unwrap_or_else(|_| id.clone());
                let Some(exe) = client.open_subkey(r"shell\open\command").ok().and_then(|c| c.get_value::<String, _>("").ok()) else { continue };
                // `"C:\…\browser.exe" --args` or an unquoted path.
                let exe = exe.strip_prefix('"').and_then(|e| e.split('"').next()).unwrap_or(exe.split(" -").next().unwrap_or(&exe)).trim();
                let exe = PathBuf::from(exe);
                if exe.is_file() && !list.iter().any(|(_, e)| e == &exe) {
                    list.push((name, exe));
                }
            }
        }
        list
    }

    /// A well-known browser's executable, where its installer usually puts it.
    pub fn find(key: &str) -> Option<PathBuf> {
        let (exe, known): (&str, &[&str]) = match key {
            "firefox" => ("firefox.exe", &[r"Mozilla Firefox\firefox.exe"]),
            "waterfox" => (
                "waterfox.exe",
                &[r"Waterfox\waterfox.exe", r"Waterfox Current\waterfox.exe", r"Waterfox Classic\waterfox.exe", r"Programs\Waterfox\waterfox.exe"],
            ),
            "chrome" => ("chrome.exe", &[r"Google\Chrome\Application\chrome.exe"]),
            "brave" => ("brave.exe", &[r"BraveSoftware\Brave-Browser\Application\brave.exe"]),
            "opera" => ("opera.exe", &[r"Programs\Opera\opera.exe", r"Programs\Opera\launcher.exe", r"Opera\launcher.exe"]),
            "edge" => ("msedge.exe", &[r"Microsoft\Edge\Application\msedge.exe"]),
            "chromium" => ("chromium.exe", &[r"Chromium\Application\chrome.exe"]),
            "vivaldi" => ("vivaldi.exe", &[r"Vivaldi\Application\vivaldi.exe"]),
            "librewolf" => ("librewolf.exe", &[r"LibreWolf\librewolf.exe"]),
            _ => return None,
        };
        let registered = [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().find_map(|hive| {
            let key = RegKey::predef(hive).open_subkey(format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{exe}")).ok()?;
            let path: String = key.get_value("").ok()?;
            Some(PathBuf::from(path.trim_matches('"')))
        });
        registered.filter(|p| p.is_file()).or_else(|| {
            ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"]
                .into_iter()
                .filter_map(std::env::var_os)
                .flat_map(|root| known.iter().map(move |k| PathBuf::from(&root).join(k)))
                .find(|p| p.is_file())
        })
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::PathBuf;

    /// The applications of this computer that are browsers (whatever their name), with their bundle.
    pub fn registered() -> Vec<(String, PathBuf)> {
        let home = directories::BaseDirs::new().map(|d| d.home_dir().join("Applications"));
        std::iter::once(PathBuf::from("/Applications"))
            .chain(home)
            .filter_map(|dir| std::fs::read_dir(dir).ok())
            .flat_map(|apps| apps.flatten().map(|app| app.path()))
            .filter(|app| app.extension().is_some_and(|e| e == "app") && super::flavour_of(app).is_some())
            .filter_map(|app| Some((app.file_stem()?.to_string_lossy().into_owned(), app)))
            .collect()
    }

    pub fn find(key: &str) -> Option<PathBuf> {
        registered().into_iter().find(|(name, _)| super::key_of(name) == key).map(|(_, app)| app)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::path::PathBuf;

    /// Linux has no list of installed browsers: the well-known ones are looked for by name.
    pub fn registered() -> Vec<(String, PathBuf)> {
        Vec::new()
    }

    pub fn find(key: &str) -> Option<PathBuf> {
        let names: &[&str] = match key {
            "firefox" => &["firefox", "firefox-esr", "org.mozilla.firefox"],
            "waterfox" => &["waterfox", "waterfox-g", "waterfox-current", "net.waterfox.waterfox"],
            "librewolf" => &["librewolf", "io.gitlab.librewolf-community"],
            "floorp" => &["floorp", "one.ablaze.floorp"],
            "zen" => &["zen", "zen-browser", "app.zen_browser.zen"],
            "mullvad" => &["mullvad-browser", "net.mullvad.MullvadBrowser"],
            "chrome" => &["google-chrome", "google-chrome-stable", "com.google.Chrome"],
            "brave" => &["brave-browser", "brave", "com.brave.Browser"],
            "opera" => &["opera", "com.opera.Opera"],
            "edge" => &["microsoft-edge", "microsoft-edge-stable", "com.microsoft.Edge"],
            "vivaldi" => &["vivaldi", "vivaldi-stable", "com.vivaldi.Vivaldi"],
            "chromium" => &["chromium", "chromium-browser", "org.chromium.Chromium"],
            _ => return None,
        };
        let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
        let dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            // Waterfox's own tarball unpacks to /opt/waterfox.
            .chain(["/snap/bin", "/var/lib/flatpak/exports/bin", "/opt/waterfox"].map(PathBuf::from))
            .chain(home.map(|h| h.join(".local/share/flatpak/exports/bin")))
            .collect();
        names.iter().flat_map(|n| dirs.iter().map(move |d| d.join(n))).find(|p| p.is_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same cases as `browserKey` in `extension/test/shared.test.mjs`.
    #[test]
    fn a_browsers_key_is_the_first_telling_word_of_its_name() {
        for (name, key) in [
            ("Google Chrome", "chrome"),
            ("Microsoft Edge", "edge"),
            ("Mozilla Firefox", "firefox"),
            ("Firefox", "firefox"),
            ("Waterfox", "waterfox"),
            ("Mullvad Browser", "mullvad"),
            ("Zen Browser", "zen"),
            ("Opera Stable", "opera"),
            ("Brave", "brave"),
            ("Thorium", "thorium"),
            ("Supercalifragilisticexpialidocious", "supercalifragili"),
            ("", ""),
        ] {
            assert_eq!(key_of(name), key, "{name}");
        }
    }

    #[test]
    fn the_extension_skips_the_same_generic_words() {
        let shared_js = include_str!("../../../../extension/shared.js");
        let start = shared_js.find("const NOT_A_NAME").unwrap();
        let list = &shared_js[start..start + shared_js[start..].find("]);").unwrap()];
        let mut words: Vec<&str> = list.split('"').skip(1).step_by(2).collect();
        let mut ours = NOT_A_NAME.to_vec();

        words.sort_unstable();
        ours.sort_unstable();

        assert_eq!(words, ours);
    }

    #[test]
    fn a_browser_is_known_by_its_key_whatever_it_is() {
        let firefox = Browser::from_key("firefox").unwrap();
        assert_eq!((firefox.name(), firefox.flavour()), ("Firefox", Flavour::Firefox));
        let thorium = Browser::from_key("thorium").unwrap();
        assert_eq!((thorium.name(), thorium.flavour(), thorium.extensions_page()), ("Thorium", Flavour::Chromium, "chrome://extensions/"));
        assert_eq!(Browser::from_key("thorium"), Some(thorium), "the same browser again");
        assert!(Browser::from_key("Net Scape").is_none() && Browser::from_key("").is_none());
    }
}
