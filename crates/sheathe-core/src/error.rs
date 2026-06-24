//! The crate-wide error type, re-exported from the workspace root.

/// Convenient alias used throughout the sheathe crates.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced while parsing, packaging, or muxing media.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An underlying I/O failure.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// The input bitstream or container was malformed.
    #[error("malformed input: {0}")]
    Malformed(String),

    /// A feature, codec, or container variant is recognised but not yet implemented.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// A configuration or CLI argument was invalid.
    #[error("invalid configuration: {0}")]
    Config(String),
}

impl Error {
    /// Construct a [`Error::Malformed`] from anything string-like.
    pub fn malformed(msg: impl Into<String>) -> Self {
        Error::Malformed(msg.into())
    }

    /// Construct an [`Error::Unsupported`] from anything string-like.
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Error::Unsupported(msg.into())
    }
}
