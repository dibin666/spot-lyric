use std::{fmt, str::FromStr};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

use crate::{
    error::{DaemonError, Result},
    types::{LyricsCandidate, StoredLyricsCandidate},
};

const CANDIDATE_HANDLE_PREFIX: &str = "lyricsc1_";

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $( $variant, )+
        }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $value, )+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = DaemonError;

            fn from_str(value: &str) -> Result<Self> {
                match value {
                    $( $value => Ok(Self::$variant), )+
                    _ => Err(DaemonError::InvalidArgument(format!(
                        "invalid {}: {value}",
                        stringify!($name),
                    ))),
                }
            }
        }
    };
}

string_enum!(ShuffleMode {
    Off => "off",
    Context => "context",
    Smart => "smart",
});

string_enum!(RepeatMode {
    Off => "off",
    Context => "context",
    Track => "track",
});

string_enum!(LyricsProviderPreference {
    Netease => "netease",
    Qq => "qq",
});

string_enum!(SettingsChangeScope {
    Auth => "auth",
    Lyrics => "lyrics",
    Cache => "cache",
});

pub fn encode_candidate_id(candidate: &StoredLyricsCandidate) -> Result<String> {
    let payload = serde_json::to_vec(candidate)?;
    Ok(format!(
        "{CANDIDATE_HANDLE_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(payload)
    ))
}

pub fn decode_candidate_id(candidate_id: &str) -> Result<StoredLyricsCandidate> {
    let encoded = candidate_id
        .strip_prefix(CANDIDATE_HANDLE_PREFIX)
        .ok_or_else(|| DaemonError::InvalidCandidateId(candidate_id.to_string()))?;
    let payload = URL_SAFE_NO_PAD.decode(encoded)?;
    Ok(serde_json::from_slice(&payload)?)
}

pub fn to_public_lyrics_candidate(candidate: &StoredLyricsCandidate) -> Result<LyricsCandidate> {
    Ok(LyricsCandidate {
        candidate_id: encode_candidate_id(candidate)?,
        album: candidate.album.clone(),
        artists: candidate.artists.clone(),
        duration_ms: candidate.duration_ms,
        provider: candidate.provider.clone(),
        score: candidate.score,
        title: candidate.title.clone(),
    })
}
