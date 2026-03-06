use thiserror::Error;

/// Core error type for the Nebo application.
#[derive(Debug, Error)]
pub enum NeboError {
    // Auth errors
    #[error("user not found")]
    UserNotFound,
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("email already exists")]
    EmailExists,
    #[error("invalid or expired token")]
    InvalidToken,
    #[error("unauthorized")]
    Unauthorized,

    // Database errors
    #[error("database error: {0}")]
    Database(String),
    #[error("not found")]
    NotFound,
    #[error("migration error: {0}")]
    Migration(String),

    // Config errors
    #[error("config error: {0}")]
    Config(String),
    #[error("data directory error: {0}")]
    DataDir(String),

    // Server errors
    #[error("port {0} is already in use - only one Nebo instance allowed per computer")]
    PortInUse(u16),
    #[error("server error: {0}")]
    Server(String),

    // AI provider errors
    #[error("context overflow")]
    ContextOverflow,
    #[error("rate limit exceeded")]
    RateLimit,

    // IO errors
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    // Validation
    #[error("validation error: {0}")]
    Validation(String),

    // Generic
    #[error("{0}")]
    Internal(String),
}

impl NeboError {
    /// HTTP status code for this error.
    pub fn status_code(&self) -> u16 {
        match self {
            Self::UserNotFound | Self::NotFound => 404,
            Self::InvalidCredentials | Self::Unauthorized | Self::InvalidToken => 401,
            Self::EmailExists => 409,
            Self::RateLimit => 429,
            Self::ContextOverflow => 413,
            _ => 500,
        }
    }
}
