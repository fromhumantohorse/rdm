//! The HTTP client of downloads, and how any client reaches servers: directly, through the system's
//! proxy, or through a given one.

use std::{sync::Arc, time::Duration};

use reqwest::Client;

use crate::net;

const MAX_REDIRECTS: usize = 10;

/// How the engine reaches servers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum Route {
    /// Direct connections, even if the environment defines a proxy.
    Direct,
    /// The system's proxy (Windows and macOS settings, `HTTP(S)_PROXY` / `ALL_PROXY` variables).
    #[default]
    System,
    /// This proxy: `http://`, `https://`, `socks5://`, `socks5h://` (names resolved by the proxy),
    /// `socks4://`, `socks4a://`; with a login when `user` is not empty.
    Proxy { url: String, user: String, password: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ClientOptions {
    pub route: Route,
    /// Accept invalid TLS certificates (a download the user explicitly exempted).
    pub insecure: bool,
    /// A download that starts on the Internet: no connection into the local network, whatever the
    /// names it meets resolve to (`net::PublicDns`). Not through a proxy, which resolves the
    /// names itself (and may well be on the local network).
    pub public_only: bool,
}

/// The default client: system proxy, certificates checked.
pub fn client() -> reqwest::Result<Client> {
    client_with(&ClientOptions::default())
}

/// Tuned for bulk parallel transfers:
/// - HTTP/1.1 only: HTTP/2 would multiplex every segment onto one TCP stream and kill parallelism;
/// - no compression: `Content-Encoding` on ranged responses breaks offsets, and media is already compressed;
/// - kept-alive pool sized for the max connection count, so stolen segments reuse warm TLS sessions;
/// - names resolved once per minute and shared (`net::CachedDns`): 32 connections to one server
///   cost one DNS lookup, not 32 (a slow resolver no longer delays every connection);
/// - redirects may not lead from the Internet into the local network (SSRF), and with
///   `public_only` no name may resolve into it either.
///
/// Privacy: no telemetry, no update ping; a browser-like User-Agent so the client is not fingerprinted as RDM.
pub fn client_with(options: &ClientOptions) -> reqwest::Result<Client> {
    let redirects = reqwest::redirect::Policy::custom(|attempt| {
        let hops = attempt.previous().len();
        let escapes = attempt.previous().last().is_some_and(|from| !net::allowed_hop(from, attempt.url()));
        if hops > MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if escapes {
            attempt.error("redirect into the local network blocked")
        } else {
            attempt.follow()
        }
    });
    let mut builder = Client::builder()
        .http1_only()
        .tcp_nodelay(true)
        .tcp_keepalive(Duration::from_secs(30))
        .pool_max_idle_per_host(usize::from(domain::MAX_CONNECTIONS))
        .pool_idle_timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(30))
        .dns_resolver(net::CachedDns::shared())
        .redirect(redirects)
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36");
    if options.public_only && !matches!(options.route, Route::Proxy { .. }) {
        builder = builder.dns_resolver(Arc::new(net::PublicDns));
    }
    builder = route(builder, &options.route)?;
    if options.insecure {
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder.build()
}

/// `builder` set up to reach servers by `route` (for RDM's other clients too: updates, VirusTotal).
pub fn route(builder: reqwest::ClientBuilder, route: &Route) -> reqwest::Result<reqwest::ClientBuilder> {
    Ok(match route {
        Route::Direct => builder.no_proxy(),
        Route::System => builder,
        Route::Proxy { url, user, password } => {
            let mut proxy = reqwest::Proxy::all(url.as_str())?;
            // SOCKS4 has no password authentication (reqwest panics if asked for one).
            let socks4 = url.get(..6).is_some_and(|s| s.eq_ignore_ascii_case("socks4"));
            if !user.is_empty() && !socks4 {
                proxy = proxy.basic_auth(user, password);
            }
            builder.no_proxy().proxy(proxy)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_proxy_kind_builds_with_or_without_a_login() {
        for url in ["http://p:3128", "https://p:3128", "socks5://p:1080", "socks5h://p:1080", "socks4://p:1080", "SOCKS4A://p:1080"] {
            for user in ["", "me"] {
                let route = Route::Proxy { url: url.into(), user: user.into(), password: "pw".into() };
                assert!(client_with(&ClientOptions { route, ..ClientOptions::default() }).is_ok(), "{url} {user}");
            }
        }
    }
}
