use thiserror::Error;

pub type Result<T, E = DaemonError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("could not resolve user config directory")]
    MissingConfigDir,
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("auth error: {0}")]
    Auth(String),
    #[error("invalid lyrics candidate id: {0}")]
    InvalidCandidateId(String),
    #[error("synchronization poisoned: {0}")]
    Poisoned(String),
    #[error("spotify {method} {url} -> {status}: {response_text}")]
    HttpStatus {
        status: u16,
        method: String,
        url: String,
        response_text: String,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    #[error(transparent)]
    Zbus(#[from] zbus::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
    #[error(transparent)]
    Base64(#[from] base64::DecodeError),
}
