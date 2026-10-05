//! The checksum a user expects for a download (published next to it by its site): MD5, SHA-1,
//! SHA-256 or SHA-512, read once from what the user pasted, kept as `algo:hex`.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgo {
    Md5,
    Sha1,
    Sha256,
    Sha512,
}

impl HashAlgo {
    const ALL: [Self; 4] = [Self::Md5, Self::Sha1, Self::Sha256, Self::Sha512];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Md5 => "md5",
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
        }
    }

    /// Hexadecimal digits of its digest.
    const fn hex_len(self) -> usize {
        match self {
            Self::Md5 => 32,
            Self::Sha1 => 40,
            Self::Sha256 => 64,
            Self::Sha512 => 128,
        }
    }

    fn of_len(len: usize) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.hex_len() == len)
    }

    fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.name() == name)
    }
}

/// A digest and its algorithm: always a known algorithm and the lower-case hex digits it makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checksum {
    algo: HashAlgo,
    hex: String,
}

impl Checksum {
    /// `input` as the user pasted it (`SHA256: AB CD…`, `md5:…`, a bare hex string, `hash  file.iso`);
    /// `None` when it is not a checksum.
    pub fn parse(input: &str) -> Option<Self> {
        let s = input.trim();
        let (label, rest) = match s.split_once(':') {
            Some((label, rest)) if label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') => {
                (Some(label.to_ascii_lowercase().replace('-', "")), rest)
            }
            _ => (None, s),
        };
        // `sha256sum` output: the hash, then the file name.
        let hex: String = rest.split_whitespace().take_while(|w| w.chars().all(|c| c.is_ascii_hexdigit())).collect::<String>().to_ascii_lowercase();
        let algo = HashAlgo::of_len(hex.len())?;
        if label.is_some_and(|l| l != algo.name()) {
            return None; // "sha256:" with 32 hex digits: a typo, not an MD5
        }
        Some(Self { algo, hex })
    }

    /// The `algo:hex` form it is saved in.
    fn saved(text: &str) -> Option<Self> {
        let (name, hex) = text.split_once(':')?;
        let algo = HashAlgo::named(name)?;
        let well_formed = hex.len() == algo.hex_len() && hex.bytes().all(|b| b.is_ascii_hexdigit());
        well_formed.then(|| Self { algo, hex: hex.to_ascii_lowercase() })
    }

    pub const fn algo(&self) -> HashAlgo {
        self.algo
    }

    /// Whether a file's digest (hex, any case) is this one.
    pub fn matches(&self, digest: &str) -> bool {
        digest.eq_ignore_ascii_case(&self.hex)
    }

    /// A saved download's checksum; one this version cannot read is dropped, not the download.
    pub(crate) fn deserialize_saved<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Self>, D::Error> {
        Ok(Option::<String>::deserialize(deserializer)?.as_deref().and_then(Self::saved))
    }
}

impl fmt::Display for Checksum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algo.name(), self.hex)
    }
}

impl Serialize for Checksum {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn recognises_what_users_paste() {
        let expected = format!("sha256:{EMPTY_SHA256}");
        for pasted in [EMPTY_SHA256.to_owned(), format!("SHA-256: {}", EMPTY_SHA256.to_uppercase()), format!("{EMPTY_SHA256}  ubuntu.iso")] {
            assert_eq!(Checksum::parse(&pasted).map(|c| c.to_string()), Some(expected.clone()), "{pasted}");
        }
        assert_eq!(Checksum::parse("MD5:d41d8cd98f00b204e9800998ecf8427e").map(|c| c.algo()), Some(HashAlgo::Md5));
    }

    #[test]
    fn a_digest_followed_by_the_files_address_is_still_read() {
        let pasted = format!("{EMPTY_SHA256}  https://example.com/ubuntu.iso");

        assert_eq!(Checksum::parse(&pasted).map(|c| c.algo()), Some(HashAlgo::Sha256));
    }

    #[test]
    fn a_label_contradicting_the_length_is_a_typo_not_another_algorithm() {
        assert_eq!(Checksum::parse("sha256:d41d8cd98f00b204e9800998ecf8427e"), None);
    }

    #[test]
    fn text_that_is_no_digest_is_refused() {
        assert_eq!(Checksum::parse("not a hash"), None);
        assert_eq!(Checksum::parse("abc"), None);
    }

    #[test]
    fn a_digest_matches_whatever_its_case() {
        let checksum = Checksum::parse(EMPTY_SHA256).unwrap();

        assert!(checksum.matches(&EMPTY_SHA256.to_uppercase()));
        assert!(!checksum.matches(&EMPTY_SHA256.replace('e', "f")));
    }

    #[test]
    fn it_is_saved_as_algo_colon_hex_and_read_back() {
        let checksum = Checksum::parse(EMPTY_SHA256).unwrap();

        let saved = serde_json::to_string(&checksum).unwrap();

        assert_eq!(saved, format!("\"sha256:{EMPTY_SHA256}\""));
        assert_eq!(Checksum::saved(&format!("sha256:{EMPTY_SHA256}")), Some(checksum));
    }

    #[test]
    fn a_saved_checksum_must_be_whole() {
        assert_eq!(Checksum::saved("sha256:abc"), None, "too short");
        assert_eq!(Checksum::saved("crc32:cbf43926"), None, "unknown algorithm");
        assert_eq!(Checksum::saved(&format!("sha256:{}", "z".repeat(64))), None, "not hexadecimal");
    }
}
