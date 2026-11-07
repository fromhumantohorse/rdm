//! Passwords — site logins, the proxy's, the VirusTotal API key — kept apart from `settings.json`:
//! on Windows encrypted with DPAPI (only this Windows account can decrypt them), on Linux and macOS
//! in a file only the user can read (0600, like `~/.netrc`). Never shown in the list, never sent
//! anywhere but to their site.

use std::{io, path::Path};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::settings::{self, Settings};

const FILE: &str = "secrets.bin";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Secrets {
    pub sites: Vec<SiteLogin>,
    pub proxy_password: String,
    /// The user's own (free) VirusTotal API key; empty = not set up.
    pub virustotal_key: String,
    /// What Safari's extension shows RDM once approved: its origin changes at each launch of Safari.
    pub safari_pair: String,
}

/// HTTP login for a site: sent (Basic) to `host` and its subdomains.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SiteLogin {
    pub host: String,
    pub user: String,
    pub password: String,
}

impl Secrets {
    pub fn load() -> Self {
        Self::load_from(&settings::config_file(FILE))
    }

    /// A file that cannot be opened (damaged, or sealed for another Windows account) is kept aside
    /// (`secrets.bin.bad`): the next save must not erase the passwords it may still hold.
    fn load_from(path: &Path) -> Self {
        let Some(sealed) = settings::read_config(path) else { return Self::default() };
        match imp::open(&sealed).and_then(|plain| serde_json::from_slice(&plain).ok()) {
            Some(secrets) => secrets,
            None => {
                settings::keep_aside(path);
                Self::default()
            }
        }
    }

    pub fn save(&self) -> io::Result<()> {
        self.save_to(&settings::config_file(FILE))
    }

    fn save_to(&self, path: &Path) -> io::Result<()> {
        let plain = serde_json::to_vec(self)?;
        let refused = || io::Error::other(crate::tr!("le système a refusé de chiffrer les mots de passe", "the system refused to encrypt the passwords"));
        let sealed = imp::seal(&plain).ok_or_else(refused)?;
        if let Some(dir) = path.parent() {
            settings::create_private_dir(dir)?;
        }
        let tmp = settings::with_suffix(path, ".tmp");
        let written = settings::write_private(&tmp, &sealed).and_then(|()| std::fs::rename(&tmp, path));
        // Passwords left behind under another name: in clear, outside Windows.
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written
    }

    /// RDM 0.3.7 and older kept the VirusTotal key in the settings, in clear: it moves here once
    /// stored (`store`), and only then leaves the settings. `true` when the settings changed.
    pub fn adopt_virustotal_key(&mut self, settings: &mut Settings, store: impl FnOnce(&Self) -> io::Result<()>) -> bool {
        if settings.virustotal_key.is_empty() {
            return false;
        }
        if self.virustotal_key.is_empty() {
            self.virustotal_key = settings.virustotal_key.clone();
            if store(self).is_err() {
                return false; // kept in the settings meanwhile: not lost
            }
        }
        settings.virustotal_key.clear();
        true
    }

    /// The login saved for `url`'s host (or a parent domain), the most specific first.
    pub fn login_for(&self, url: &Url) -> Option<&SiteLogin> {
        let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
        self.sites
            .iter()
            .filter(|s| !s.user.is_empty())
            .filter_map(|s| {
                let pattern = normalize_host(&s.host);
                let matches = !pattern.is_empty() && (host == pattern || host.ends_with(&format!(".{pattern}")));
                matches.then_some((pattern.len(), s))
            })
            .max_by_key(|(specificity, _)| *specificity)
            .map(|(_, s)| s)
    }
}

/// `https://Example.com:8080/x` → `example.com`: what a user may paste as a site.
pub fn normalize_host(input: &str) -> String {
    let s = input.trim();
    let s = s.split_once("://").map_or(s, |(_, rest)| rest);
    let s = s.split(['/', '?', '#']).next().unwrap_or_default();
    let s = s.rsplit_once('@').map_or(s, |(_, host)| host);
    // `[::1]:80` → `[::1]` (as `Url::host_str` writes an IPv6 host); `host:port` → `host`.
    let s = if s.starts_with('[') { s.find(']').map_or(s, |end| &s[..=end]) } else { s.split(':').next().unwrap_or_default() };
    s.trim_end_matches('.').to_ascii_lowercase()
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData},
    };

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr().cast_mut() }
    }

    /// DPAPI output buffer → owned bytes, freed.
    unsafe fn take(out: &CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        // SAFETY: DPAPI returned `cbData` bytes at `pbData` (not null), allocated with LocalAlloc.
        unsafe {
            let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(out.pbData.cast());
            bytes
        }
    }

    pub fn seal(plain: &[u8]) -> Option<Vec<u8>> {
        let input = blob(plain);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: valid input blob for the call's duration; `out` receives a LocalAlloc buffer.
        let ok = unsafe {
            CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        // SAFETY: `out` was filled by a successful call.
        (ok != 0).then(|| unsafe { take(&out) })
    }

    pub fn open(sealed: &[u8]) -> Option<Vec<u8>> {
        let input = blob(sealed);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: as in `seal`.
        let ok = unsafe {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        // SAFETY: `out` was filled by a successful call.
        (ok != 0).then(|| unsafe { take(&out) })
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn seal(plain: &[u8]) -> Option<Vec<u8>> {
        Some(plain.to_vec())
    }

    pub fn open(sealed: &[u8]) -> Option<Vec<u8>> {
        Some(sealed.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_normalized() {
        assert_eq!(normalize_host("https://User@Files.Example.com:8443/a?b"), "files.example.com");
        assert_eq!(normalize_host(" example.com. "), "example.com");
        assert_eq!(normalize_host("[::1]:80"), "[::1]");
    }

    #[test]
    fn the_most_specific_login_wins_and_lookalikes_do_not_match() {
        let login = |host: &str, user: &str| SiteLogin { host: host.into(), user: user.into(), password: "p".into() };
        let s = Secrets { sites: vec![login("example.com", "a"), login("files.example.com", "b"), login("", "c")], ..Default::default() };
        let url = |u: &str| u.parse::<Url>().unwrap();
        assert_eq!(s.login_for(&url("https://files.example.com/x")).unwrap().user, "b");
        assert_eq!(s.login_for(&url("https://cdn.example.com/x")).unwrap().user, "a");
        assert!(s.login_for(&url("https://notexample.com/x")).is_none());
        assert!(s.login_for(&url("https://example.com.evil.io/x")).is_none());
        // Written with its scheme, the more specific site still wins.
        let s = Secrets { sites: vec![login("http://files.example.com", "b"), login("example.com", "a")], ..Default::default() };
        assert_eq!(s.login_for(&url("http://files.example.com/x")).unwrap().user, "b");
    }

    #[test]
    fn the_virustotal_key_leaves_the_settings_once_stored() {
        let key = "a".repeat(64);
        let with_key = || Settings { virustotal_key: key.clone(), ..Settings::default() };
        let (mut settings, mut secrets) = (with_key(), Secrets::default());
        assert!(!secrets.adopt_virustotal_key(&mut settings, |_| Err(io::ErrorKind::PermissionDenied.into())), "not stored: stays where it was");
        assert_eq!(settings.virustotal_key, key);
        let mut settings = with_key();
        assert!(Secrets::default().adopt_virustotal_key(&mut settings, |s| {
            assert_eq!(s.virustotal_key, key);
            Ok(())
        }));
        assert!(settings.virustotal_key.is_empty());
        assert!(!serde_json::to_string(&settings).unwrap().contains("virustotal_key"), "no longer written in clear");
        // Already moved (an older RDM ran again meanwhile): the settings' copy just goes.
        let (mut settings, mut secrets) = (with_key(), Secrets { virustotal_key: "b".repeat(64), ..Secrets::default() });
        assert!(secrets.adopt_virustotal_key(&mut settings, |_| panic!("nothing to store")));
        assert_eq!(secrets.virustotal_key, "b".repeat(64));
    }

    /// Regression: a file RDM could not open was taken for "no passwords", and the next save
    /// erased whatever it held.
    #[test]
    fn an_unreadable_file_is_kept_aside_not_erased() {
        let dir = std::env::temp_dir().join(format!("rdm-secrets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        std::fs::write(&path, b"\x00 sealed for another account").unwrap();

        let loaded = Secrets::load_from(&path);

        assert_eq!(loaded, Secrets::default());
        assert_eq!(std::fs::read(settings::with_suffix(&path, ".bad")).unwrap(), b"\x00 sealed for another account");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_passwords_load_back() {
        let dir = std::env::temp_dir().join(format!("rdm-secrets-round-{}", std::process::id()));
        let path = dir.join(FILE);
        let secrets = Secrets { proxy_password: "s3cret".into(), virustotal_key: "k".repeat(64), ..Secrets::default() };

        secrets.save_to(&path).unwrap();

        assert_eq!(Secrets::load_from(&path), secrets);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sealing_round_trips() {
        let plain = b"{\"proxy_password\":\"s3cret\"}";
        let sealed = imp::seal(plain).unwrap();
        if cfg!(windows) {
            assert!(!sealed.windows(6).any(|w| w == b"s3cret"), "not stored in clear");
        }
        assert_eq!(imp::open(&sealed).unwrap(), plain);
    }
}
