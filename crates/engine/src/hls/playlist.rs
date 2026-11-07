//! Reading playlists (RFC 8216): a master's qualities and their audio renditions, a media
//! playlist's segments with their keys, byte ranges and initialization section.

use std::{collections::HashMap, sync::Arc};

use serde::Serialize;
use url::Url;

use super::{MAX_PART_BYTES, MAX_PARTS, PlaylistError};
use crate::EngineError;

/// One quality of a master playlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Variant {
    pub url: Url,
    /// Separate audio rendition to fetch and mux alongside (`EXT-X-MEDIA:TYPE=AUDIO`).
    pub audio: Option<Url>,
    pub bandwidth: u64,
    pub height: Option<u32>,
}

pub(super) struct Key {
    pub(super) url: Url,
    pub(super) iv: Option<[u8; 16]>,
}

pub(super) struct Part {
    pub(super) url: Url,
    /// Shared: a long stream has a hundred thousand segments, and most often one key.
    pub(super) key: Option<Arc<Key>>,
    pub(super) seq: u64,
    /// `EXT-X-BYTERANGE`: (offset, length) inside `url`.
    pub(super) range: Option<(u64, u64)>,
}

pub(super) enum Playlist {
    Master(Vec<Variant>),
    Media { parts: Vec<Part>, fmp4: bool },
}

pub(super) fn parse(base: &Url, text: &str) -> Result<Playlist, EngineError> {
    if !text.trim_start_matches('\u{feff}').trim_start().starts_with("#EXTM3U") {
        return Err(PlaylistError::NotAPlaylist.into());
    }
    if text.contains("#EXT-X-STREAM-INF") {
        return Ok(Playlist::Master(parse_master(base, text)));
    }
    let parts = parse_media(base, text)?;
    Ok(Playlist::Media { parts, fmp4: text.contains("#EXT-X-MAP") })
}

fn parse_master(base: &Url, text: &str) -> Vec<Variant> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    // GROUP-ID → the audio rendition a player takes: the DEFAULT one, else the first. One without
    // a URI is the sound inside the variant's own segments: no separate track then.
    let mut audio: HashMap<&str, (bool, Option<Url>)> = HashMap::new();
    for attrs in lines.iter().filter_map(|l| l.strip_prefix("#EXT-X-MEDIA:")) {
        if attr(attrs, "TYPE") != Some("AUDIO") {
            continue;
        }
        let Some(group) = attr(attrs, "GROUP-ID") else { continue };
        let url = match attr(attrs, "URI").map(|u| base.join(u)) {
            Some(Ok(url)) => Some(url),
            Some(Err(_)) => continue,
            None => None,
        };
        let default = attr(attrs, "DEFAULT") == Some("YES");
        if audio.get(group).is_none_or(|(was_default, _)| default && !was_default) {
            audio.insert(group, (default, url));
        }
    }

    let mut variants = Vec::new();
    let mut pending: Option<&str> = None;
    for line in &lines {
        if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending = Some(attrs);
        } else if !line.is_empty()
            && !line.starts_with('#')
            && let (Some(attrs), Ok(url)) = (pending.take(), base.join(line))
        {
            variants.push(Variant {
                url,
                audio: attr(attrs, "AUDIO").and_then(|g| audio.get(g)).and_then(|(_, u)| u.clone()),
                bandwidth: attr(attrs, "BANDWIDTH").and_then(|b| b.parse().ok()).unwrap_or(0),
                height: attr(attrs, "RESOLUTION").and_then(|r| r.split_once(['x', 'X'])?.1.parse().ok()),
            });
        }
    }
    variants.sort_by_key(|v| std::cmp::Reverse((v.height, v.bandwidth)));
    variants.dedup_by(|a, b| a.url == b.url);
    variants
}

fn parse_media(base: &Url, text: &str) -> Result<Vec<Part>, EngineError> {
    let join = |u: &str| base.join(u).map_err(|_| PlaylistError::BadSegmentUrl);
    let (mut parts, mut key, mut seq) = (Vec::new(), None, 0u64);
    // `EXT-X-BYTERANGE` without `@offset` continues right after the previous range of the same resource.
    let mut pending_range: Option<(u64, Option<u64>)> = None;
    let mut last_end: Option<(Url, u64)> = None;
    for line in text.lines().map(str::trim) {
        if let Some(v) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            seq = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            pending_range = Some(parse_byterange(v).ok_or(PlaylistError::BadByteRange)?);
        } else if let Some(a) = line.strip_prefix("#EXT-X-KEY:") {
            key = match attr(a, "METHOD") {
                Some("NONE") => None,
                Some("AES-128") => Some(Arc::new(Key {
                    url: join(attr(a, "URI").ok_or(PlaylistError::KeyWithoutUri)?)?,
                    iv: attr(a, "IV").map(|iv| parse_iv(iv).ok_or(PlaylistError::InvalidKey)).transpose()?,
                })),
                _ => return Err(PlaylistError::Drm.into()),
            };
        } else if let Some(a) = line.strip_prefix("#EXT-X-MAP:") {
            let uri = attr(a, "URI").ok_or(PlaylistError::MapWithoutUri)?;
            let range = match attr(a, "BYTERANGE") {
                Some(r) => {
                    let (len, offset) = parse_byterange(r).ok_or(PlaylistError::BadByteRange)?;
                    let offset = offset.unwrap_or(0);
                    offset.checked_add(len).ok_or(PlaylistError::BadByteRange)?;
                    Some((offset, len))
                }
                None => None,
            };
            // After a key, the section is encrypted with it, and RFC 8216 then requires its IV:
            // written as is, it would make a "complete" file no player can read.
            if key.as_ref().is_some_and(|key| key.iv.is_none()) {
                return Err(PlaylistError::InvalidKey.into());
            }
            parts.push(Part { url: join(uri)?, key: key.clone(), seq, range });
        } else if !line.is_empty() && !line.starts_with('#') {
            let url = join(line)?;
            let range = match pending_range.take() {
                None => {
                    last_end = None;
                    None
                }
                Some((len, offset)) => {
                    let offset = match (offset, &last_end) {
                        (Some(o), _) => o,
                        (None, Some((prev, end))) if *prev == url => *end,
                        (None, _) => return Err(PlaylistError::ByteRangeWithoutOffset.into()),
                    };
                    let end = offset.checked_add(len).ok_or(PlaylistError::BadByteRange)?;
                    last_end = Some((url.clone(), end));
                    Some((offset, len))
                }
            };
            parts.push(Part { url, key: key.clone(), seq, range });
            seq = seq.wrapping_add(1);
        }
        if parts.len() > MAX_PARTS {
            return Err(PlaylistError::TooManySegments.into());
        }
    }
    if parts.is_empty() {
        return Err(PlaylistError::Empty.into());
    }
    Ok(parts)
}

/// `length[@offset]`.
fn parse_byterange(v: &str) -> Option<(u64, Option<u64>)> {
    let (len, offset) = match v.trim().split_once('@') {
        Some((l, o)) => (l, Some(o.trim().parse().ok()?)),
        None => (v.trim(), None),
    };
    let len: u64 = len.trim().parse().ok()?;
    (len > 0 && len <= MAX_PART_BYTES).then_some((len, offset))
}

/// `KEY=value,KEY="quoted, value"` attribute lists.
fn attr<'a>(list: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = list;
    while !rest.is_empty() {
        let (key, after) = rest.split_once('=')?;
        let (value, next) = match after.strip_prefix('"') {
            Some(quoted) => {
                let (v, n) = quoted.split_once('"')?;
                (v, n.strip_prefix(',').unwrap_or(n))
            }
            None => after.split_once(',').unwrap_or((after, "")),
        };
        if key.trim() == name {
            return Some(value);
        }
        rest = next;
    }
    None
}

fn parse_iv(hex: &str) -> Option<[u8; 16]> {
    let hex = hex.strip_prefix("0x").or_else(|| hex.strip_prefix("0X"))?;
    u128::from_str_radix(hex, 16).ok().map(u128::to_be_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        "https://cdn.io/v/master.m3u8".parse().unwrap()
    }

    fn master(text: &str) -> Vec<Variant> {
        match parse(&base(), text).unwrap() {
            Playlist::Master(v) => v,
            Playlist::Media { .. } => panic!("expected master"),
        }
    }

    #[test]
    fn orders_variants_best_first() {
        let v = master(
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\nlow.m3u8\n\
             #EXT-X-STREAM-INF:BANDWIDTH=5000000,CODECS=\"avc1,mp4a\",RESOLUTION=1920x1080\nhd/high.m3u8\n",
        );
        assert_eq!(v[0].url.as_str(), "https://cdn.io/v/hd/high.m3u8");
        assert_eq!((v[0].height, v[1].height), (Some(1080), Some(360)));
    }

    #[test]
    fn links_audio_renditions_preferring_default() {
        let v = master(
            "#EXTM3U\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"en\",URI=\"en.m3u8\"\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"fr\",DEFAULT=YES,URI=\"fr.m3u8\"\n\
             #EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"sub\",URI=\"subs.m3u8\"\n\
             #EXT-X-STREAM-INF:BANDWIDTH=900,AUDIO=\"aud\",RESOLUTION=1280x720\nv720.m3u8\n\
             #EXT-X-STREAM-INF:BANDWIDTH=500\nmuxed.m3u8\n",
        );
        assert_eq!(v[0].audio.as_ref().unwrap().as_str(), "https://cdn.io/v/fr.m3u8");
        assert!(v[1].audio.is_none());
    }

    /// Regression (Apple's `bipbop_16x9`): the DEFAULT audio rendition has no URI — its sound is in
    /// the variant — and an alternate one has; RDM took the alternate and refused the stream.
    #[test]
    fn a_default_rendition_without_uri_is_the_variants_own_sound() {
        let v = master(
            "#EXTM3U\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"Main\",AUTOSELECT=YES,DEFAULT=YES\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"Alt\",DEFAULT=NO,URI=\"alt/prog.m3u8\"\n\
             #EXT-X-STREAM-INF:BANDWIDTH=900,CODECS=\"mp4a.40.2,avc1.4d400d\",AUDIO=\"a\"\ngear1/prog.m3u8\n",
        );
        assert!(v[0].audio.is_none(), "{:?}", v[0].audio);
    }

    #[test]
    fn parses_segments_keys_and_map() {
        let text = "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:7\n#EXT-X-MAP:URI=\"init.mp4\"\n\
                    #EXT-X-KEY:METHOD=AES-128,URI=\"k.bin\",IV=0x0000000000000000000000000000000A\n\
                    #EXTINF:4,\na.ts\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:4,\nb.ts\n#EXT-X-ENDLIST\n";
        let Playlist::Media { parts, fmp4 } = parse(&base(), text).unwrap() else { panic!("expected media") };
        assert!(fmp4);
        assert_eq!(parts.len(), 3);
        assert!(parts[0].key.is_none());
        assert_eq!(parts[1].key.as_ref().unwrap().iv.unwrap()[15], 10);
        assert_eq!((parts[1].seq, parts[2].seq), (7, 8));
        assert!(parts[2].key.is_none());
    }

    #[test]
    fn an_initialization_section_after_a_key_is_decrypted_with_it() {
        let parts = |text: &str| match parse(&base(), text) {
            Ok(Playlist::Media { parts, .. }) => parts,
            _ => panic!("expected media"),
        };
        let with_iv = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k.bin\",IV=0x0A\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4,\na.m4s\n";
        let without_iv = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k.bin\"\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4,\na.m4s\n";

        assert_eq!(parts(with_iv)[0].key.as_ref().unwrap().iv.unwrap()[15], 10);
        assert!(
            matches!(parse(&base(), without_iv), Err(EngineError::Playlist(PlaylistError::InvalidKey))),
            "an encrypted map without an IV cannot be decoded"
        );
        let malformed_iv = with_iv.replace("0x0A", "0xwrong");
        assert!(
            matches!(parse(&base(), &malformed_iv), Err(EngineError::Playlist(PlaylistError::InvalidKey))),
            "never silently substitute the sequence number for an invalid IV"
        );
    }

    #[test]
    fn byte_ranges_follow_each_other() {
        let text = "#EXTM3U\n#EXT-X-MAP:URI=\"main.mp4\",BYTERANGE=\"616@0\"\n\
                    #EXTINF:6,\n#EXT-X-BYTERANGE:1000@616\nmain.mp4\n\
                    #EXTINF:6,\n#EXT-X-BYTERANGE:500\nmain.mp4\n";
        let Playlist::Media { parts, .. } = parse(&base(), text).unwrap() else { panic!("expected media") };
        let ranges: Vec<_> = parts.iter().map(|p| p.range).collect();
        assert_eq!(ranges, [Some((0, 616)), Some((616, 1000)), Some((1616, 500))]);
        let orphan = "#EXTM3U\n#EXTINF:6,\n#EXT-X-BYTERANGE:500\nmain.mp4\n";
        assert!(parse(&base(), orphan).is_err());
        let interrupted = "#EXTM3U\n#EXT-X-BYTERANGE:10@0\nmain.mp4\nother.ts\n#EXT-X-BYTERANGE:10\nmain.mp4\n";
        assert!(
            matches!(parse(&base(), interrupted), Err(EngineError::Playlist(PlaylistError::ByteRangeWithoutOffset))),
            "only the immediately preceding segment supplies an implicit offset"
        );
        assert!(parse_byterange("0@5").is_none());
        assert!(parse_byterange("18446744073709551615@1").is_none());
        let overflow = "#EXTM3U\n#EXT-X-MAP:URI=\"main.mp4\",BYTERANGE=\"616@18446744073709551615\"\n#EXTINF:6,\nmain.mp4\n";
        assert!(parse(&base(), overflow).is_err(), "an init segment past the end of any file");
    }

    #[test]
    fn refuses_endless_playlists() {
        let mut text = String::from("#EXTM3U\n");
        text.push_str(&"#EXTINF:1,\na.ts\n".repeat(MAX_PARTS + 1));
        assert!(matches!(parse(&base(), &text), Err(EngineError::Playlist(PlaylistError::TooManySegments))));
    }

    #[test]
    fn rejects_drm_and_garbage() {
        let drm = "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://x\"\n#EXTINF:4,\na.ts\n";
        assert!(parse(&base(), drm).is_err());
        assert!(parse(&base(), "<html>").is_err());
        assert!(parse(&base(), "#EXTM3U\n").is_err());
        assert!(attr("A=\"unterminated", "A").is_none());
    }
}
