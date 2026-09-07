use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// The file is not an ISO-BMFF container, or a box header is malformed.
    Malformed(&'static str),
    /// The container parsed cleanly but holds no JPEG preview we can serve.
    NoPreview,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Malformed(what) => write!(f, "malformed CR3: {what}"),
            Error::NoPreview => write!(f, "no embedded JPEG preview found"),
        }
    }
}

impl std::error::Error for Error {}

impl From<zaru_bmff::Error> for Error {
    fn from(e: zaru_bmff::Error) -> Self {
        match e {
            zaru_bmff::Error::Io(e) => Error::Io(e),
            zaru_bmff::Error::Malformed(what) => Error::Malformed(what),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
