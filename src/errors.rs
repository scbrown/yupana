//! Error and result types for Yupana.

use thiserror::Error;

/// The crate-wide error type.
#[derive(Debug, Error)]
pub enum Error {
    /// An underlying I/O failure.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// Configuration could not be loaded or parsed.
    #[error("config error: {0}")]
    Config(String),

    /// A source file could not be parsed.
    #[error("parse error: {0}")]
    Parse(String),

    /// The requested language is not supported by the current build.
    #[error("unsupported language: {0}")]
    UnsupportedLanguage(String),

    /// A Phase-4 promotion could not be validated or written (`quipu` feature).
    #[error("promote error: {0}")]
    Promote(String),

    /// A Phase-4 policy projection could not be fetched or decoded from quipu
    /// (`quipu` feature).
    #[error("projection error: {0}")]
    Projection(String),
}

/// The crate-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;
