//! Why a download stopped, and what that says about trying again.

use crate::{PlaylistError, mux, net};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("server ignored range request")]
    RangeIgnored,
    #[error("server sent no data")]
    Empty,
    #[error("connection closed before the end")]
    Truncated,
    /// A connection stopped receiving while the others did: its piece goes to another one.
    #[error("connection stalled")]
    Stalled,
    #[error("blocked: Internet content pointing into the local network")]
    LocalNetwork,
    #[error("playlist: {0}")]
    Playlist(#[from] PlaylistError),
    #[error("merge: {0}")]
    Mux(#[from] mux::MuxError),
}

impl EngineError {
    /// Connecting failed: the cached address of that server is forgotten, so the next attempt
    /// resolves its name again (the network changed, a VPN went up, the CDN moved).
    pub(crate) fn forget_address(&self) {
        if let Self::Http(e) = self
            && e.is_connect()
            && let Some(url) = e.url()
        {
            net::forget(url);
        }
    }

    /// 429 / 503: the server limits concurrent connections.
    pub(crate) fn is_throttled(&self) -> bool {
        matches!(self, Self::Http(e) if e.status().is_some_and(|s| s.as_u16() == 429 || s.as_u16() == 503))
    }

    /// The server's TLS certificate is not trusted (self-signed, expired, another name).
    pub fn is_certificate(&self) -> bool {
        let Self::Http(e) = self else { return false };
        let mut source: Option<&dyn std::error::Error> = Some(e);
        while let Some(err) = source {
            if err.to_string().to_ascii_lowercase().contains("certificate") {
                return true;
            }
            source = err.source();
        }
        false
    }

    /// Retrying cannot help (404, 403, blocked redirect, untrusted certificate, disk error…).
    pub fn is_permanent(&self) -> bool {
        match self {
            Self::Http(e) => {
                e.is_redirect()
                    || e.is_builder()
                    || e.status().is_some_and(|s| s.is_client_error() && !matches!(s.as_u16(), 408 | 425 | 429))
                    || self.is_certificate()
                    || net::is_lan_blocked(e)
            }
            Self::Io(_) | Self::Empty | Self::LocalNetwork | Self::Playlist(_) | Self::Mux(_) => true,
            Self::RangeIgnored | Self::Truncated | Self::Stalled => false,
        }
    }
}
