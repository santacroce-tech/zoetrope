use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// A referenced symbol, layer or element does not exist.
    NotFound(String),
    /// The requested edit or the project structure is invalid.
    Invalid(String),
    /// The file is not a parseable Zoetrope project.
    Format(String),
    /// The file was written by a newer (or unknown) schema version.
    UnsupportedVersion { found: u64, supported: u32 },
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(what) => write!(f, "not found: {what}"),
            Error::Invalid(why) => write!(f, "invalid: {why}"),
            Error::Format(why) => write!(f, "bad project file: {why}"),
            Error::UnsupportedVersion { found, supported } => write!(
                f,
                "project schema version {found} is not supported (this build reads up to {supported})"
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Format(e.to_string())
    }
}
