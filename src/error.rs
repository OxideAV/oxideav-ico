//! Crate-local error type used by `oxideav-ico`'s standalone (no
//! `oxideav-core`) public API.
//!
//! When the `registry` feature is enabled, [`IcoError`] gains a
//! `From<IcoError> for oxideav_core::Error` impl (defined in
//! [`crate::registry`]) so the trait-side surface (`Decoder` /
//! `Encoder`) can keep returning `oxideav_core::Result<T>` while the
//! standalone container parser stays framework-free.

use core::fmt;

/// `Result` alias scoped to `oxideav-ico`. Standalone (no `oxideav-core`)
/// callers see this; framework callers convert via the gated
/// `From<IcoError> for oxideav_core::Error` impl.
pub type Result<T> = core::result::Result<T, IcoError>;

/// Error variants returned by `oxideav-ico`'s standalone API.
///
/// The variants mirror the subset of `oxideav_core::Error` the parser
/// can hit. The crate intentionally avoids surfacing transport (`Io`)
/// or framework-specific (`FormatNotFound`, `CodecNotFound`) errors —
/// those originate in callers that are already linking `oxideav-core`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IcoError {
    /// The byte stream is malformed (bad magic, truncated directory,
    /// entry payload spans past EOF, …).
    InvalidData(String),
    /// The byte stream uses a feature this codec doesn't implement
    /// (currently never produced by the standalone parser, kept for
    /// future-proofing).
    Unsupported(String),
}

impl IcoError {
    /// Construct an [`IcoError::InvalidData`] from a stringy message.
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidData(msg.into())
    }

    /// Construct an [`IcoError::Unsupported`] from a stringy message.
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::Unsupported(msg.into())
    }
}

impl fmt::Display for IcoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(s) => write!(f, "invalid data: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

impl std::error::Error for IcoError {}
