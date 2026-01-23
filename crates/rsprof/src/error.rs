use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Process not found: {0}")]
    ProcessNotFound(String),

    #[error("Multiple processes match '{pattern}':\n{matches}Use --pid to specify exactly one.")]
    MultipleProcesses { pattern: String, matches: String },

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Missing debug info in {path}. Recompile with `debug = true` in Cargo.toml")]
    MissingDebugInfo { path: String },

    #[error("perf_event error: {0}")]
    PerfEvent(String),

    #[error("Sampler error: {0}")]
    Sampler(String),

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Symbol resolution error: {0}")]
    SymbolResolution(String),

    #[error("Unsupported platform: {0}")]
    UnsupportedPlatform(String),
}

pub type Result<T> = std::result::Result<T, Error>;

// Exit codes as per RFC
pub mod exit_code {
    pub const SUCCESS: i32 = 0;
    pub const GENERAL_ERROR: i32 = 1;
    pub const INVALID_ARGUMENTS: i32 = 2;
    pub const PROCESS_NOT_FOUND: i32 = 3;
    pub const PERMISSION_DENIED: i32 = 4;
    pub const MISSING_DEBUG_INFO: i32 = 5;
    pub const DATABASE_ERROR: i32 = 6;
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::ProcessNotFound(_) | Error::MultipleProcesses { .. } => {
                exit_code::PROCESS_NOT_FOUND
            }
            Error::PermissionDenied(_) => exit_code::PERMISSION_DENIED,
            Error::MissingDebugInfo { .. } => exit_code::MISSING_DEBUG_INFO,
            Error::Database(_) => exit_code::DATABASE_ERROR,
            Error::InvalidArgument(_) => exit_code::INVALID_ARGUMENTS,
            _ => exit_code::GENERAL_ERROR,
        }
    }
}
