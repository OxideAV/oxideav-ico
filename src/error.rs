//! Crate-local error type used by every `oxideav-ico` entry point.
//!
//! When the `registry` feature is enabled, [`IcoError`] gains a
//! `From<IcoError> for oxideav_core::Error` impl (defined in
//! [`crate::registry`]) so the trait-side surface (`Decoder` /
//! `Encoder`) can keep returning `oxideav_core::Result<T>` while the
//! standalone API stays framework-free.

use core::fmt;

/// `Result` alias scoped to `oxideav-ico`.
pub type Result<T> = core::result::Result<T, IcoError>;

/// Contract alias: every image crate exposes its error enum as `Error`.
pub type Error = IcoError;

/// Error variants returned by `oxideav-ico`.
///
/// The four variants are the image-crate contract floor
/// (`IMAGE_CRATE_API`). Sub-image decode / encode failures from the
/// `oxideav-bmp` and `oxideav-png` crates are mapped variant-for-variant
/// (their messages are kept verbatim behind an `ICO:` prefix).
#[derive(Debug)]
#[non_exhaustive]
pub enum IcoError {
    /// The byte stream is malformed (bad magic, truncated directory,
    /// entry payload spans past EOF, corrupt sub-image, …).
    InvalidData(String),
    /// The byte stream uses a feature this crate doesn't implement, or
    /// an encode request cannot be represented by the format (an image
    /// larger than 256 px, a layout the writer has no encoder for).
    Unsupported(String),
    /// A [`crate::DecodeOptions`] limit (dimensions / pixels / bytes)
    /// would be exceeded; nothing was allocated.
    LimitExceeded(String),
    /// A read / write on a caller-supplied stream failed
    /// ([`crate::decode_from`] / [`crate::encode_to`]).
    Io(std::io::Error),
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

    /// Construct an [`IcoError::LimitExceeded`] from a stringy message.
    pub fn limit(msg: impl Into<String>) -> Self {
        Self::LimitExceeded(msg.into())
    }
}

impl fmt::Display for IcoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(s) => write!(f, "invalid data: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
            Self::LimitExceeded(s) => write!(f, "limit exceeded: {s}"),
            Self::Io(e) => write!(f, "i/o error: {e}"),
        }
    }
}

impl std::error::Error for IcoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for IcoError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<oxideav_bmp::BmpError> for IcoError {
    fn from(e: oxideav_bmp::BmpError) -> Self {
        use oxideav_bmp::BmpError as B;
        match e {
            B::InvalidData(s) => Self::InvalidData(format!("ICO: DIB sub-image: {s}")),
            B::Unsupported(s) => Self::Unsupported(format!("ICO: DIB sub-image: {s}")),
            B::LimitExceeded(s) => Self::LimitExceeded(format!("ICO: DIB sub-image: {s}")),
            B::Io(e) => Self::Io(e),
            other => Self::InvalidData(format!("ICO: DIB sub-image: {other}")),
        }
    }
}

impl From<oxideav_png::PngError> for IcoError {
    fn from(e: oxideav_png::PngError) -> Self {
        use oxideav_png::PngError as P;
        match e {
            P::InvalidData(s) => Self::InvalidData(format!("ICO: PNG sub-image: {s}")),
            P::Unsupported(s) => Self::Unsupported(format!("ICO: PNG sub-image: {s}")),
            P::LimitExceeded(s) => Self::LimitExceeded(format!("ICO: PNG sub-image: {s}")),
            P::Io(e) => Self::Io(e),
            other => Self::InvalidData(format!("ICO: PNG sub-image: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_prefixes_each_variant() {
        assert_eq!(IcoError::invalid("x").to_string(), "invalid data: x");
        assert_eq!(IcoError::unsupported("x").to_string(), "unsupported: x");
        assert_eq!(IcoError::limit("x").to_string(), "limit exceeded: x");
        let io: IcoError = std::io::Error::other("disk").into();
        assert!(matches!(io, IcoError::Io(_)));
        assert!(io.to_string().contains("disk"));
    }

    #[test]
    fn sub_image_errors_map_variant_for_variant() {
        let b: IcoError = oxideav_bmp::BmpError::limit("big").into();
        assert!(matches!(b, IcoError::LimitExceeded(ref s) if s.contains("DIB")));
        let p: IcoError = oxideav_png::PngError::unsupported("deep").into();
        assert!(matches!(p, IcoError::Unsupported(ref s) if s.contains("PNG")));
    }
}
