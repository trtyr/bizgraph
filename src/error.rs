use std::{fmt, path::PathBuf};

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    Http(reqwest::Error),
    Uuid(uuid::Error),
    Chrono(chrono::ParseError),
    Toml(toml::de::Error),
    IoContext {
        context: String,
        source: std::io::Error,
    },
    SqliteContext {
        context: String,
        source: rusqlite::Error,
    },
    JsonContext {
        context: String,
        source: serde_json::Error,
    },
    TomlContext {
        context: String,
        source: toml::de::Error,
    },
    MissingNode {
        kind: &'static str,
        key: String,
    },
    ProjectNotFound {
        reference: String,
    },
    ProjectAlreadyExists {
        name: String,
    },
    AmbiguousProject {
        reference: String,
        matches: Vec<String>,
    },
    EmptyProjectName,
    EmptyProjectReference,
    ApiRequest {
        context: String,
        source: reqwest::Error,
    },
    ApiResponse {
        status: reqwest::StatusCode,
        body: String,
        url: String,
    },
    ApiResponseDecode {
        context: String,
        source: reqwest::Error,
    },
    BudgetExceeded {
        scope: String,
        used: usize,
        limit: usize,
    },
    TaskPanicked {
        task: String,
        details: String,
    },
    ConfigMissingApiKey,
    Validation {
        message: String,
    },
    InvalidNodeKind {
        value: String,
    },
    InvalidUuidValue {
        value: String,
        source: uuid::Error,
    },
    InvalidTimestampValue {
        value: String,
        source: chrono::ParseError,
    },
    ConfigRead {
        path: PathBuf,
        source: std::io::Error,
    },
    ConfigParse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::IoContext {
            context: context.into(),
            source,
        }
    }

    pub fn sqlite(context: impl Into<String>, source: rusqlite::Error) -> Self {
        Self::SqliteContext {
            context: context.into(),
            source,
        }
    }

    pub fn json(context: impl Into<String>, source: serde_json::Error) -> Self {
        Self::JsonContext {
            context: context.into(),
            source,
        }
    }

    pub fn toml(context: impl Into<String>, source: toml::de::Error) -> Self {
        Self::TomlContext {
            context: context.into(),
            source,
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(source) => write!(f, "I/O error: {source}"),
            Self::Sqlite(source) => write!(f, "SQLite error: {source}"),
            Self::Json(source) => write!(f, "JSON error: {source}"),
            Self::Http(source) => write!(f, "HTTP error: {source}"),
            Self::Uuid(source) => write!(f, "UUID error: {source}"),
            Self::Chrono(source) => write!(f, "time parse error: {source}"),
            Self::Toml(source) => write!(f, "TOML parse error: {source}"),
            Self::IoContext { context, source } => write!(f, "{context}: {source}"),
            Self::SqliteContext { context, source } => write!(f, "{context}: {source}"),
            Self::JsonContext { context, source } => write!(f, "{context}: {source}"),
            Self::TomlContext { context, source } => write!(f, "{context}: {source}"),
            Self::MissingNode { kind, key } => write!(f, "missing {kind} node for {key}"),
            Self::ProjectNotFound { reference } => write!(f, "project '{reference}' not found"),
            Self::ProjectAlreadyExists { name } => write!(f, "project '{name}' already exists"),
            Self::AmbiguousProject { reference, matches } => write!(
                f,
                "project reference '{}' is ambiguous: {}",
                reference,
                matches.join(", ")
            ),
            Self::EmptyProjectName => write!(f, "project name cannot be empty"),
            Self::EmptyProjectReference => write!(f, "project name or id cannot be empty"),
            Self::ApiRequest { context, source } => write!(f, "{context}: {source}"),
            Self::ApiResponse { status, body, url } => {
                let truncated = if body.len() > 200 {
                    format!("{}...", &body[..200])
                } else {
                    body.clone()
                };
                write!(f, "AI API error ({status}) at {url}: {truncated}")
            }
            Self::ApiResponseDecode { context, source } => write!(f, "{context}: {source}"),
            Self::BudgetExceeded { scope, used, limit } => {
                write!(
                    f,
                    "{scope} exceeded budget ({used}/{limit}). \
                     Try analyzing a HAR with fewer endpoints, or increase MAX_DEEP_AI_CALLS."
                )
            }
            Self::TaskPanicked { task, details } => write!(f, "{task} panicked: {details}"),
            Self::ConfigMissingApiKey => write!(
                f,
                "API key not found. Configure api_key in ~/.config/bizgraph/config.toml"
            ),
            Self::Validation { message } => write!(f, "{message}"),
            Self::InvalidNodeKind { value } => write!(f, "unknown business node kind '{value}'"),
            Self::InvalidUuidValue { value, source } => {
                write!(f, "invalid uuid '{value}': {source}")
            }
            Self::InvalidTimestampValue { value, source } => {
                write!(f, "invalid timestamp '{value}': {source}")
            }
            Self::ConfigRead { path, source } => {
                write!(f, "Failed to read config file {}: {source}", path.display())
            }
            Self::ConfigParse { path, source } => {
                write!(f, "Failed to parse config file {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(source) => Some(source),
            Self::Sqlite(source) => Some(source),
            Self::Json(source) => Some(source),
            Self::Http(source) => Some(source),
            Self::Uuid(source) => Some(source),
            Self::Chrono(source) => Some(source),
            Self::Toml(source) => Some(source),
            Self::IoContext { source, .. } => Some(source),
            Self::SqliteContext { source, .. } => Some(source),
            Self::JsonContext { source, .. } => Some(source),
            Self::TomlContext { source, .. } => Some(source),
            Self::ApiRequest { source, .. } => Some(source),
            Self::ApiResponseDecode { source, .. } => Some(source),
            Self::InvalidUuidValue { source, .. } => Some(source),
            Self::InvalidTimestampValue { source, .. } => Some(source),
            Self::ConfigRead { source, .. } => Some(source),
            Self::ConfigParse { source, .. } => Some(source),
            Self::MissingNode { .. }
            | Self::ProjectNotFound { .. }
            | Self::ProjectAlreadyExists { .. }
            | Self::AmbiguousProject { .. }
            | Self::EmptyProjectName
            | Self::EmptyProjectReference
            | Self::ApiResponse { .. }
            | Self::BudgetExceeded { .. }
            | Self::TaskPanicked { .. }
            | Self::ConfigMissingApiKey
            | Self::Validation { .. }
            | Self::InvalidNodeKind { .. } => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Self::Io(source)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(source: rusqlite::Error) -> Self {
        Self::Sqlite(source)
    }
}

impl From<serde_json::Error> for Error {
    fn from(source: serde_json::Error) -> Self {
        Self::Json(source)
    }
}

impl From<reqwest::Error> for Error {
    fn from(source: reqwest::Error) -> Self {
        Self::Http(source)
    }
}

impl From<uuid::Error> for Error {
    fn from(source: uuid::Error) -> Self {
        Self::Uuid(source)
    }
}

impl From<chrono::ParseError> for Error {
    fn from(source: chrono::ParseError) -> Self {
        Self::Chrono(source)
    }
}

impl From<toml::de::Error> for Error {
    fn from(source: toml::de::Error) -> Self {
        Self::Toml(source)
    }
}
