//! Who may use the bridge (see the threat model in `bridge`): `Host`, `Origin`, and for a local
//! program without `Origin`, the token from the user's private files.

use axum::http::{
    HeaderMap, StatusCode,
    header::{HOST, ORIGIN},
};

use super::{EXTENSION_ORIGIN, STORE_EXTENSION_ORIGIN};
use crate::{local, manager::Manager};

/// Sent by the extension: `firefox`, `waterfox`, `chrome`, `brave`, `opera`, `edge` or `chromium`.
const BROWSER: &str = "x-rdm-browser";

pub(super) fn guard(manager: &Manager, headers: &HeaderMap) -> Result<(), StatusCode> {
    guard_access(manager, headers, Access::Act)
}

/// What a request carrying no `Origin` may do without the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Access {
    /// Only learn what any program of the computer may know (RDM is here, the formats it takes):
    /// the Firefox extension's `GET`s carry no `Origin`.
    Read,
    /// Anything else: the browsers' own origins, or a local program holding the token.
    Act,
}

pub(super) fn guard_access(manager: &Manager, headers: &HeaderMap, access: Access) -> Result<(), StatusCode> {
    let token = local::served_token().unwrap_or_default();
    guard_with(headers, token, access, |origin| manager.firefox_allowed(origin))?;
    // Which browser the extension runs in (the extension window shows the connected ones); sent
    // by the extension itself or its native connector. A web page cannot set this header; an
    // anonymous read (any program of the computer) is not taken at its word either.
    let proven = headers.contains_key(ORIGIN) || guard_with(headers, token, Access::Act, |_| false).is_ok();
    if proven && let Some(browser) = browser_of(headers) {
        manager.browser_seen(browser);
    }
    Ok(())
}

/// `firefox`: whether a (`moz-extension://…`) origin was approved by the user. Such an origin not
/// approved (yet) gets 401 — the extension then says "approve me in RDM", not "RDM is not
/// running" — anything else untrusted gets 403.
fn guard_with(headers: &HeaderMap, token: &str, access: Access, firefox: impl FnOnce(&str) -> bool) -> Result<(), StatusCode> {
    let host_ok = headers.get(HOST).and_then(|h| h.to_str().ok()).is_some_and(|h| {
        let host = h.rsplit_once(':').map_or(h, |(host, _)| host);
        matches!(host, "127.0.0.1" | "localhost")
    });
    if !host_ok {
        return Err(StatusCode::FORBIDDEN);
    }
    match headers.get(ORIGIN).map(|o| o.to_str()) {
        // The Firefox extension's reads: they say which browser they come from. A web page's
        // simple GET carries no `Origin` either, but cannot add that header (it would need a CORS
        // preflight, never answered): it cannot even learn that RDM is installed.
        None if access == Access::Read && browser_of(headers).is_some_and(crate::extension::valid_key) => Ok(()),
        // A local program: only the user's own (it could read the token in the user's files).
        None if !token.is_empty() && local::token_matches(token, headers.get(local::TOKEN_HEADER).map(|v| v.as_bytes())) => Ok(()),
        None => Err(StatusCode::FORBIDDEN),
        Some(Ok(o)) if o == EXTENSION_ORIGIN || o == STORE_EXTENSION_ORIGIN => Ok(()),
        Some(Ok(o)) if o.starts_with("moz-extension://") => {
            if firefox(o) { Ok(()) } else { Err(StatusCode::UNAUTHORIZED) }
        }
        Some(_) => Err(StatusCode::FORBIDDEN),
    }
}

/// The browser the extension says it runs in.
pub(super) fn browser_of(headers: &HeaderMap) -> Option<&str> {
    headers.get(BROWSER).and_then(|v| v.to_str().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORBIDDEN: Result<(), StatusCode> = Err(StatusCode::FORBIDDEN);

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        pairs.iter().map(|(k, v)| (k.parse().unwrap(), v.parse().unwrap())).collect()
    }

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn origin_and_host_rules() {
        let guard = |h: &HeaderMap| guard_with(h, TOKEN, Access::Act, |_| panic!("not a Firefox origin"));
        assert_eq!(guard(&headers(&[("host", "127.0.0.1:9614"), ("origin", EXTENSION_ORIGIN)])), Ok(()));
        assert_eq!(guard(&headers(&[("host", "127.0.0.1:9614"), ("origin", STORE_EXTENSION_ORIGIN)])), Ok(()));
        assert_eq!(guard(&headers(&[("host", "localhost:9614"), (local::TOKEN_HEADER, TOKEN)])), Ok(()));
        assert_eq!(guard(&headers(&[("host", "127.0.0.1:9614"), ("origin", "https://evil.example")])), FORBIDDEN);
        assert_eq!(guard(&headers(&[("host", "127.0.0.1:9614"), ("origin", "chrome-extension://another")])), FORBIDDEN);
        assert_eq!(guard(&headers(&[("host", "evil.example:9614"), ("origin", EXTENSION_ORIGIN)])), FORBIDDEN);
        assert_eq!(guard(&headers(&[("host", "127.0.0.1.evil.example"), (local::TOKEN_HEADER, TOKEN)])), FORBIDDEN);
        assert_eq!(guard(&headers(&[])), FORBIDDEN);
    }

    /// A local program without `Origin` must prove it is the user's (the token from the user's
    /// files): another account of the computer, or a stripped `Origin`, gets nothing done.
    #[test]
    fn local_programs_need_the_token() {
        let h = |pairs: &[(&'static str, &str)]| headers(&[&[("host", "127.0.0.1:9614")], pairs].concat());
        let act = |h: &HeaderMap| guard_with(h, TOKEN, Access::Act, |_| panic!("no origin"));
        assert_eq!(act(&h(&[(local::TOKEN_HEADER, TOKEN)])), Ok(()));
        assert_eq!(act(&h(&[])), FORBIDDEN, "no token");
        assert_eq!(act(&h(&[(local::TOKEN_HEADER, &TOKEN.replace('0', "1"))])), FORBIDDEN, "a wrong token");
        assert_eq!(guard_with(&h(&[]), "", Access::Act, |_| false), FORBIDDEN, "no token issued: nobody");
        // Reading whether RDM is here and what it captures: the Firefox extension's GETs (they
        // name their browser) or the token; not a web page's bare GET (it could tell RDM is here).
        assert_eq!(guard_with(&h(&[(BROWSER, "firefox")]), TOKEN, Access::Read, |_| false), Ok(()));
        assert_eq!(guard_with(&h(&[(local::TOKEN_HEADER, TOKEN)]), TOKEN, Access::Read, |_| false), Ok(()));
        assert_eq!(guard_with(&h(&[]), TOKEN, Access::Read, |_| false), FORBIDDEN, "a web page's simple GET");
        assert_eq!(guard_with(&h(&[(BROWSER, "Not A Key!")]), TOKEN, Access::Read, |_| false), FORBIDDEN);
        // An origin is judged as an origin, token or not.
        let web = h(&[("origin", "https://evil.example"), (local::TOKEN_HEADER, TOKEN)]);
        assert_eq!(guard_with(&web, TOKEN, Access::Read, |_| false), FORBIDDEN);
    }

    #[test]
    fn firefox_origins_need_approval() {
        const FF: &str = "moz-extension://0f8e7a1c-3b2d-4e5f-9a8b-7c6d5e4f3a2b";
        let h = headers(&[("host", "127.0.0.1:9614"), ("origin", FF)]);
        assert_eq!(guard_with(&h, TOKEN, Access::Act, |o| o == FF), Ok(()));
        // Waiting for the user's approval: 401, told apart from a foreign caller.
        assert_eq!(guard_with(&h, TOKEN, Access::Act, |_| false), Err(StatusCode::UNAUTHORIZED));
        // A foreign host is refused before anything is put up for approval.
        let h = headers(&[("host", "evil.example"), ("origin", FF)]);
        assert_eq!(guard_with(&h, TOKEN, Access::Act, |_| panic!("must not ask")), FORBIDDEN);
    }
}
