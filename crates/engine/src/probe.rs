use std::time::Duration;

use reqwest::{
    Client, Response, StatusCode,
    header::{self, HeaderMap, HeaderValue},
};
use url::Url;

use crate::EngineError;

/// The identity tried when a server cuts off the browser-like one: some servers (mirrors,
/// speed-test hosts) drop any "Mozilla/…" User-Agent that does not come from a real browser, and
/// take the common download tools'.
pub const FALLBACK_AGENT: &str = "Wget/1.25.0";

#[derive(Debug, Clone)]
pub struct Probe {
    pub size: Option<u64>,
    pub ranges: bool,
    pub file_name: String,
    /// HLS playlist: downloaded segment by segment (see [`crate::hls_info`] for the container).
    pub hls: bool,
    /// What identifies this version of the file on the server (`Last-Modified`, else a strong
    /// `ETag`): a download resumed later must continue the same file, not a newer one.
    pub version: Option<String>,
    /// The User-Agent the server accepted after refusing the one asked for: every request of the
    /// download must then use it.
    pub agent: Option<HeaderValue>,
}

/// The file's version as the server states it. `Last-Modified` first: it stays the same across
/// the servers of a CDN or a mirror pool, where an `ETag` often differs from one to the next
/// (Apache puts the inode in it). A weak `ETag` (`W/…`) says nothing about the bytes, and neither
/// does a date that is the moment of the answer: download scripts stamp every response "now",
/// which would make every resume start over.
pub(crate) fn version_of(headers: &HeaderMap) -> Option<String> {
    let text = |name| headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty());
    let answered = text(header::DATE).and_then(|d| httpdate::parse_http_date(d).ok());
    let stamped_now = |modified: &str| match (httpdate::parse_http_date(modified), answered) {
        // Within a minute of the answer (or after it): not a file's date.
        (Ok(modified), Some(answered)) => answered.duration_since(modified).unwrap_or_default() < std::time::Duration::from_secs(60),
        _ => false,
    };
    text(header::LAST_MODIFIED)
        .filter(|date| !stamped_now(date))
        .map(|date| format!("date:{date}"))
        .or_else(|| text(header::ETAG).filter(|tag| !tag.starts_with("W/")).map(|tag| format!("etag:{tag}")))
}

/// Attempts for the initial request: transient failures (timeout, reset, 5xx, 429) are retried.
const PROBE_ATTEMPTS: u32 = 5;

/// Longest wait between two attempts when the server asks for one (`Retry-After`).
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

/// Below this, a file comes as fast on one connection: no extra request to learn about parts.
const WORTH_PARTS: u64 = 4 << 20;

/// The most a probe's answer may hold to be read for its connection's sake (it asked for one byte).
const MAX_DRAINED: u64 = 64 << 10;

/// What the first request of a download asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    /// `bytes=0-0`: the facts only (size, range support, name).
    Facts,
    /// `bytes=0-`: the facts, and the answer kept — its body is the file from its first byte.
    Data,
    /// No `Range` at all (see [`refuses_ranges`]), the answer kept likewise.
    Plain,
}

/// One `Range: bytes=0-0` request reveals size, range support and file name at once.
pub async fn probe(client: &Client, url: &Url, headers: &HeaderMap) -> Result<Probe, EngineError> {
    first(client, url, headers, Ask::Facts).await.map(|(probe, _)| probe)
}

/// A download's first request: the facts, and the answer to read the file from — no second
/// request for its first bytes (one round trip less, and one request less for a server that
/// counts them). `None` for the answer when there is nothing to read (an empty file).
pub(crate) async fn open(client: &Client, url: &Url, headers: &HeaderMap) -> Result<(Probe, Option<Response>), EngineError> {
    first(client, url, headers, Ask::Data).await
}

async fn first(client: &Client, url: &Url, headers: &HeaderMap, ask: Ask) -> Result<(Probe, Option<Response>), EngineError> {
    let (mut attempt, mut other_tried, mut plain_tried) = (1, false, false);
    loop {
        let (err, retry_after) = match attempt_probe(client, url, headers, ask).await {
            Ok(found) => return Ok(found),
            Err(failure) => failure,
        };
        if !other_tried && refused_identity(&err) {
            other_tried = true;
            if let Some(found) = with_other_agent(client, url, headers, ask).await {
                return Ok(found);
            }
        }
        if ask == Ask::Data && !plain_tried && refuses_ranges(&err) {
            plain_tried = true;
            if let Ok(found) = attempt_probe(client, url, headers, Ask::Plain).await {
                return Ok(found);
            }
        }
        if err.is_permanent() || attempt >= PROBE_ATTEMPTS {
            return Err(err);
        }
        attempt += 1;
        let wait = Duration::from_millis(500 << attempt).max(retry_after.unwrap_or_default().min(MAX_RETRY_AFTER));
        tokio::time::sleep(wait).await;
    }
}

/// A single attempt, for when an answer is only nice to have (naming a new download quickly).
pub async fn probe_once(client: &Client, url: &Url, headers: &HeaderMap) -> Result<Probe, EngineError> {
    match attempt_probe(client, url, headers, Ask::Facts).await {
        Ok((probe, _)) => Ok(probe),
        Err((e, _)) if refused_identity(&e) => with_other_agent(client, url, headers, Ask::Facts).await.map(|(probe, _)| probe).ok_or(e),
        Err((e, _)) => Err(e),
    }
}

/// Whether the server may have refused the identity rather than the request: 403/406, or the
/// connection dropped before any answer (neither a timeout nor a failed connection).
fn refused_identity(err: &EngineError) -> bool {
    let EngineError::Http(e) = err else { return false };
    match e.status() {
        Some(status) => matches!(status.as_u16(), 403 | 406),
        None => e.is_request() && !e.is_connect() && !e.is_timeout(),
    }
}

/// Whether the server may have refused the `Range` header rather than the file: some answer an
/// error to any range, or a partial answer that cannot be used (no total, another start), where
/// they would serve the file to a plain request (on one connection then).
fn refuses_ranges(err: &EngineError) -> bool {
    match err {
        EngineError::RangeIgnored => true,
        EngineError::Http(e) => e.status().is_some_and(|s| matches!(s.as_u16(), 400 | 403 | 405 | 406 | 416 | 501)),
        _ => false,
    }
}

/// The request again as a common download tool ([`FALLBACK_AGENT`]); `None` if refused too.
async fn with_other_agent(client: &Client, url: &Url, headers: &HeaderMap, ask: Ask) -> Option<(Probe, Option<Response>)> {
    let agent = HeaderValue::from_static(FALLBACK_AGENT);
    if headers.get(header::USER_AGENT) == Some(&agent) {
        return None;
    }
    let mut other = headers.clone();
    other.insert(header::USER_AGENT, agent.clone());
    let (mut probe, res) = attempt_probe(client, url, &other, ask).await.ok()?;
    probe.agent = Some(agent);
    Some((probe, res))
}

/// One attempt; a refusal comes with the wait the server asked for (`Retry-After`).
async fn attempt_probe(client: &Client, url: &Url, headers: &HeaderMap, ask: Ask) -> Result<(Probe, Option<Response>), (EngineError, Option<Duration>)> {
    let request = client.get(url.clone()).headers(headers.clone());
    let request = match ask {
        Ask::Facts => request.header(header::RANGE, "bytes=0-0"),
        Ask::Data => request.header(header::RANGE, "bytes=0-"),
        Ask::Plain => request,
    };
    let res = request.send().await.map_err(|e| (e.into(), None))?;
    let retry_after = crate::retry_after(res.headers());
    let res = res.error_for_status().map_err(|e| (e.into(), retry_after))?;

    let h = res.headers();
    let partial = res.status() == StatusCode::PARTIAL_CONTENT;
    let size = if partial {
        let Some((0, _, total)) = crate::content_range(h) else {
            return Err((EngineError::RangeIgnored, None));
        };
        // A partial response of unknown total length cannot establish the end of the file.
        if ask != Ask::Facts && total.is_none() {
            return Err((EngineError::RangeIgnored, None));
        }
        total
    } else {
        res.content_length()
    };
    // Asked for the whole file, some servers answer with the whole file (200) although they serve
    // parts: they say so (`Accept-Ranges: bytes`).
    let announced = h.get(header::ACCEPT_RANGES).and_then(|v| v.to_str().ok()).is_some_and(|v| v.trim().eq_ignore_ascii_case("bytes"));
    let ranges = partial || (ask == Ask::Data && announced);
    let version = version_of(h);
    let disposition = h.get(header::CONTENT_DISPOSITION).map(|v| header_text(v.as_bytes()));
    let content_type = h.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok());
    let hls = crate::hls::is_playlist(res.url(), content_type);
    let mut file_name = suggest_file_name(res.url(), disposition.as_deref());
    if hls {
        let stem = file_name.rsplit_once('.').map_or(file_name.as_str(), |(s, _)| s);
        file_name = format!("{stem}.ts");
    }
    let mut probe = Probe { size, ranges: ranges && size.is_some() && !hls, file_name, hls, version, agent: None };
    if ask != Ask::Facts {
        // The whole file, without a word about parts: asked for a small part, many a server still
        // serves one (several connections are then possible). Not worth asking for a small file.
        if ask == Ask::Data && !probe.ranges && !hls && size.is_some_and(|s| s >= WORTH_PARTS) {
            probe.ranges = Box::pin(attempt_probe(client, url, headers, Ask::Facts)).await.is_ok_and(|(facts, _)| facts.ranges && facts.size == size);
        }
        return Ok((probe, Some(res)));
    }
    // Drain the 1-byte body so this warm (TLS-established) connection returns to the pool and
    // the first worker reuses it. Never for a 200 (that body would be the whole file), nor for a
    // server answering more than it was asked: read whole, it could fill the memory.
    if partial && res.content_length().is_some_and(|n| n <= MAX_DRAINED) {
        let _ = res.bytes().await;
    }
    Ok((probe, None))
}

/// A header's text: many servers send a file name in raw UTF-8 or Latin-1, as browsers read it. Taken
/// for plain ASCII, the name would be lost, and the file named after its link (`download.php`).
fn header_text(bytes: &[u8]) -> String {
    std::str::from_utf8(bytes).map_or_else(|_| bytes.iter().map(|&b| char::from(b)).collect(), str::to_owned)
}

pub fn suggest_file_name(url: &Url, disposition: Option<&str>) -> String {
    disposition
        .and_then(parse_disposition)
        .or_else(|| url.path_segments()?.next_back().filter(|s| !s.is_empty()).map(percent_decode))
        .map(|n| sanitize_file_name(&n))
        .unwrap_or_else(|| "download".into())
}

/// RFC 6266: `filename*` (RFC 8187 `charset'lang'%XX…`) wins over `filename`, in any order.
fn parse_disposition(v: &str) -> Option<String> {
    let params = disposition_params(v);
    let get = |key: &str| params.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    get("filename*").and_then(decode_ext_value).or_else(|| get("filename").map(str::to_owned)).filter(|n| !n.is_empty())
}

/// `type; key=token; key="quoted; \"value\""` → (lower-case key, value) pairs. A `;` inside quotes
/// is part of the value; parameter names are case-insensitive.
fn disposition_params(v: &str) -> Vec<(String, String)> {
    let mut params = Vec::new();
    let Some((_, mut rest)) = v.split_once(';') else { return params };
    loop {
        rest = rest.trim_start_matches([' ', '\t', ';']);
        let Some((key, after)) = rest.split_once('=') else { return params };
        let after = after.trim_start();
        let (value, next) = match after.strip_prefix('"') {
            Some(quoted) => {
                let (mut value, mut escaped, mut end) = (String::new(), false, quoted.len());
                for (i, c) in quoted.char_indices() {
                    match c {
                        _ if escaped => {
                            value.push(c);
                            escaped = false;
                        }
                        '\\' => escaped = true,
                        '"' => {
                            end = i + 1;
                            break;
                        }
                        _ => value.push(c),
                    }
                }
                (value, &quoted[end..])
            }
            None => {
                let (value, next) = after.split_once(';').unwrap_or((after, ""));
                (value.trim().to_owned(), next)
            }
        };
        params.push((key.trim().to_ascii_lowercase(), value));
        rest = next;
    }
}

/// `UTF-8''%E2%82%AC.pdf` (or ISO-8859-1) → text.
fn decode_ext_value(v: &str) -> Option<String> {
    let mut parts = v.splitn(3, '\'');
    let (charset, _lang, value) = (parts.next()?, parts.next()?, parts.next()?);
    let bytes: Vec<u8> = percent_encoding::percent_decode_str(value).collect();
    Some(if charset.eq_ignore_ascii_case("iso-8859-1") { bytes.iter().map(|&b| char::from(b)).collect() } else { String::from_utf8_lossy(&bytes).into_owned() })
}

fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned()
}

const MAX_NAME_CHARS: usize = 180;
/// Linux file systems count bytes, 255 at most: 180 Japanese or Cyrillic characters would not fit
/// (2–3 bytes each). Room is left for what RDM appends: ` (12)`, `.video.part.rdm.tmp`, `.rec….video`.
const MAX_NAME_BYTES: usize = 200;

/// Untrusted name (server, web page) → a single safe path component on Windows and Linux:
/// no separators or traversal, no reserved device names, no text-direction tricks, bounded
/// length (in characters and in bytes), extension kept.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            let unsafe_char = matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() || is_bidi_control(c);
            if unsafe_char { '_' } else { c }
        })
        .collect();
    let cleaned = cleaned.trim_matches([' ', '.']);

    let (stem, ext) = match cleaned.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.chars().count() <= 16 => (s, Some(e)),
        _ => (cleaned, None),
    };
    let chars = MAX_NAME_CHARS.saturating_sub(ext.map_or(0, |e| e.chars().count() + 1));
    let bytes = MAX_NAME_BYTES.saturating_sub(ext.map_or(0, |e| e.len() + 1));
    let mut used = 0;
    let mut stem: String = stem
        .chars()
        .take(chars)
        .take_while(|c| {
            used += c.len_utf8();
            used <= bytes
        })
        .collect::<String>()
        .trim_end()
        .to_owned();

    // Windows ignores trailing spaces here: "NUL .txt" is the NUL device too.
    let device = stem.split('.').next().unwrap_or_default().trim_end().to_uppercase();
    let numbered = |prefix: &str| {
        device.strip_prefix(prefix).is_some_and(|n| {
            let mut digits = n.chars();
            matches!((digits.next(), digits.next()), (Some('0'..='9' | '¹' | '²' | '³'), None))
        })
    };
    let reserved = matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") || numbered("COM") || numbered("LPT");
    if reserved {
        stem.insert(0, '_');
    }
    match (stem.is_empty(), ext) {
        (true, _) => "download".into(),
        (false, Some(ext)) => format!("{stem}.{ext}"),
        // A cut can leave dots at the end, where Windows drops them: the file would not be the one
        // RDM looks for afterwards.
        (false, None) => stem.trim_end_matches([' ', '.']).to_owned(),
    }
}

/// Characters that reorder the text around them: "invoice\u{202E}fdp.exe" reads "invoiceexe.pdf".
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_files_name_comes_from_its_disposition_or_its_link() {
        let u: Url = "https://x.io/dir/My%20File.zip?x=1".parse().unwrap();
        assert_eq!(suggest_file_name(&u, None), "My File.zip");
        assert_eq!(suggest_file_name(&u, Some("attachment; filename=\"a:b.iso\"")), "a_b.iso");
        assert_eq!(suggest_file_name(&u, Some("attachment; filename*=UTF-8''%C3%A9t%C3%A9.mp4")), "été.mp4");
    }

    #[test]
    fn a_files_version_is_its_real_date_or_its_strong_tag() {
        let headers = |pairs: &[(header::HeaderName, &str)]| pairs.iter().map(|(k, v)| (k.clone(), v.parse().unwrap())).collect::<HeaderMap>();
        let date = "Tue, 21 Oct 2025 07:28:00 GMT";
        assert_eq!(version_of(&headers(&[(header::ETAG, "\"abc\""), (header::LAST_MODIFIED, date)])), Some(format!("date:{date}")));
        assert_eq!(version_of(&headers(&[(header::ETAG, "\"abc\"")])).as_deref(), Some("etag:\"abc\""));
        assert_eq!(version_of(&headers(&[(header::ETAG, "W/\"abc\"")])), None, "a weak tag says nothing");
        assert_eq!(version_of(&HeaderMap::new()), None);
        // A script stamping each answer "now": the date is not the file's.
        let now = "Mon, 28 Sep 2026 12:00:00 GMT";
        assert_eq!(version_of(&headers(&[(header::DATE, now), (header::LAST_MODIFIED, now)])), None);
        assert_eq!(version_of(&headers(&[(header::DATE, now), (header::LAST_MODIFIED, now), (header::ETAG, "\"v2\"")])).as_deref(), Some("etag:\"v2\""));
        let older = format!("date:{date}");
        assert_eq!(version_of(&headers(&[(header::DATE, now), (header::LAST_MODIFIED, date)])), Some(older), "a real, older date");
    }

    /// Regression: a name in raw UTF-8 or Latin-1 was dropped, and the file named after its link.
    #[test]
    fn a_disposition_in_raw_utf8_or_latin1_keeps_its_name() {
        let u: Url = "https://x.io/download.php".parse().unwrap();
        for raw in ["attachment; filename=\"été.pdf\"".as_bytes(), b"attachment; filename=\"\xE9t\xE9.pdf\""] {
            assert_eq!(suggest_file_name(&u, Some(&header_text(raw))), "été.pdf");
        }
    }

    #[test]
    fn content_disposition_edge_cases() {
        let name = |d: &str| parse_disposition(d);
        assert_eq!(name(r#"attachment; filename="a;b.zip""#).as_deref(), Some("a;b.zip"), "';' inside quotes");
        assert_eq!(name("attachment; Filename=Report.pdf").as_deref(), Some("Report.pdf"), "case-insensitive key");
        assert_eq!(name(r#"attachment; filename="say \"hi\".txt"; size=3"#).as_deref(), Some(r#"say "hi".txt"#));
        assert_eq!(name("attachment; filename*=UTF-8''%E2%82%AC.pdf; filename=\"EUR.pdf\"").as_deref(), Some("€.pdf"));
        assert_eq!(name("attachment; filename=\"EUR.pdf\"; filename*=utf-8'en'%E2%82%AC.pdf").as_deref(), Some("€.pdf"));
        assert_eq!(name("attachment; filename*=ISO-8859-1''%E9t%E9.txt").as_deref(), Some("été.txt"));
        assert_eq!(name(r#"attachment; filename="unterminated.zip"#).as_deref(), Some("unterminated.zip"));
        assert_eq!(name("inline"), None);
        assert_eq!(name("attachment; filename=\"\""), None);
    }

    #[test]
    fn sanitizes_hostile_names() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_file_name("..\\..\\Windows\\x.exe"), "_.._Windows_x.exe");
        assert_eq!(sanitize_file_name("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_file_name("com1"), "_com1");
        assert_eq!(sanitize_file_name("NUL .txt"), "_NUL.txt");
        assert_eq!(sanitize_file_name("nul .tar.gz"), "_nul .tar.gz");
        assert_eq!(sanitize_file_name("COM¹.log"), "_COM¹.log");
        assert_eq!(sanitize_file_name("conout$"), "_conout$");
        assert_eq!(sanitize_file_name("COM10.txt"), "COM10.txt");
        assert_eq!(sanitize_file_name("Console.txt"), "Console.txt");
        assert_eq!(sanitize_file_name("invoice\u{202E}fdp.exe"), "invoice_fdp.exe", "right-to-left override");
        assert_eq!(sanitize_file_name("a\u{2067}b\u{200F}.zip"), "a_b_.zip");
        assert_eq!(sanitize_file_name(" ... "), "download");
        let long = format!("{}.mp4", "a".repeat(500));
        let safe = sanitize_file_name(&long);
        assert!(safe.chars().count() <= MAX_NAME_CHARS && safe.ends_with(".mp4"));
        // Cut right after a dot: Windows would drop it, and RDM look for a file that is not there.
        let dotted = format!("{}.{}", "a".repeat(MAX_NAME_CHARS - 1), "b".repeat(40));
        assert_eq!(sanitize_file_name(&dotted), "a".repeat(MAX_NAME_CHARS - 1));
    }

    /// A server answering far more than the one byte asked for is not read to its end.
    #[tokio::test]
    async fn a_probe_does_not_read_an_oversized_answer() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url: Url = format!("http://{}/f.bin", listener.local_addr().unwrap()).parse().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;
            let head = b"HTTP/1.1 206 Partial Content\r\ncontent-range: bytes 0-0/100\r\ncontent-length: 1073741824\r\n\r\n";
            let _ = socket.write_all(head).await;
            std::future::pending::<()>().await; // the announced gigabyte never comes
        });
        let client = crate::client_with(&crate::ClientOptions { route: crate::Route::Direct, ..crate::ClientOptions::default() }).unwrap();

        let found = tokio::time::timeout(Duration::from_secs(5), probe(&client, &url, &HeaderMap::new())).await;

        assert_eq!(found.expect("answered at once").unwrap().size, Some(100));
    }

    /// Regression (Linux: "File name too long"): a long Japanese, Cyrillic or emoji title stays
    /// within the 255 bytes of a Linux file name, with room for the temporary suffixes.
    #[test]
    fn long_names_fit_linux_file_systems() {
        for unit in ["あ", "Ж", "🎬", "a"] {
            let safe = sanitize_file_name(&format!("{}.mp4", unit.repeat(300)));
            assert!(safe.len() <= MAX_NAME_BYTES && safe.ends_with(".mp4"), "{unit}: {} bytes", safe.len());
            assert!(safe.len() + " (999)".len() + ".video.part.rdm.tmp".len() <= 255);
            assert!(safe.starts_with(unit), "never cut inside a character");
        }
        assert_eq!(sanitize_file_name("été.mp4"), "été.mp4", "short names untouched");
    }
}
