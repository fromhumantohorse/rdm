use serde::{Deserialize, Deserializer, Serialize};

/// Why a download stopped: a fact, put into words by the interface in its current language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Failure {
    /// A redirect loop, or a redirect from the Internet into the local network.
    RedirectRefused,
    /// The site's TLS certificate is not trusted (self-signed, expired, another name).
    InvalidCertificate,
    /// The server answered with this HTTP status.
    Status(u16),
    TimedOut,
    CannotConnect,
    ConnectionInterrupted,
    /// A disk error, as the system words it.
    Disk(String),
    /// The server sent another part of the file than the one asked for.
    InconsistentRange,
    /// The server sent no data: an expired or protected link.
    NoData,
    CutBeforeTheEnd,
    /// Internet content pointing into the local network.
    LocalNetwork,
    /// A video stream (HLS) that cannot be downloaded, and why (in English).
    Stream(String),
    /// The video and audio tracks could not be merged, and why (in English).
    Merge(String),
    /// The proxy address of the settings cannot be used.
    InvalidProxy,
    /// A recording RDM was closed during.
    RecordingInterrupted,
    /// A recording whose tab went silent (closed, playback stopped).
    RecordingAbandoned,
    /// A recording growing beyond any real one: the page's scripts know its token.
    RecordingTooLarge,
    RecordingCancelled,
    /// A recording that ended without both a video and an audio track.
    RecordingEmpty,
    /// Written by an older RDM, in the interface language of the time.
    Text(String),
}

impl Failure {
    /// A saved failure, of this version or of an older one (plain text).
    pub(crate) fn deserialize_saved<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Saved {
            Typed(Failure),
            Text(String),
        }
        Ok(match Saved::deserialize(deserializer)? {
            Saved::Typed(failure) => failure,
            Saved::Text(text) => Self::Text(text),
        })
    }
}
