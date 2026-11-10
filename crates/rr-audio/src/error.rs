//! Error type for the audio engine.

/// Errors produced by the audio engine.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// libopus reported an error.
    #[error("opus: {0}")]
    Opus(String),
    /// A configuration value was out of range.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
}

impl From<opus::Error> for AudioError {
    fn from(e: opus::Error) -> Self {
        Self::Opus(e.to_string())
    }
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, AudioError>;
