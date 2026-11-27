//! Typed errors returned by the library.

use std::path::PathBuf;

use crate::model::UnsupportedInput;

/// The result type used throughout the library.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong in the library. No library function panics on
/// bad input; it returns one of these.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file or directory could not be read or written.
    #[error("{action} {}: {source}", path.display())]
    Io {
        /// What was being attempted, such as "read" or "create".
        action: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// A manifest, lockfile or configuration file could not be understood.
    #[error("invalid {kind} at {}: {message}", path.display())]
    Parse {
        /// The kind of file, such as "manifest" or "lockfile".
        kind: &'static str,
        /// The file involved.
        path: PathBuf,
        /// What was wrong.
        message: String,
    },

    /// The package manager exited with an error.
    #[error("`{command}` failed: {stderr}")]
    Tool {
        /// The command line that was run.
        command: String,
        /// What the tool printed on standard error.
        stderr: String,
    },

    /// The package manager could not be started.
    #[error("could not run `{command}`: {source}")]
    ToolUnavailable {
        /// The command line that was attempted.
        command: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// A command outside the safety allow-list was requested.
    #[error(
        "refusing to run `{0}`: only resolution commands may run on a candidate dependency set"
    )]
    ToolNotAllowed(String),

    /// The store rejected an operation.
    #[error("store error ({gql_status}): {message}")]
    Store {
        /// Relate's message.
        message: String,
        /// The GQLSTATUS code Relate reported.
        gql_status: String,
    },

    /// Another process holds the store for writing.
    #[error("the store is in use by another process: {0}")]
    StoreLocked(String),

    /// The project has no store yet.
    #[error("no depvigil store at {}; run `depvigil init` first", .0.display())]
    NotInitialised(PathBuf),

    /// The workspace no longer matches the recorded baseline.
    #[error("the workspace has changed since the baseline was recorded; run `depvigil init` to record the new baseline")]
    StaleBaseline,

    /// The workspace uses inputs depvigil cannot simulate safely.
    #[error("unsupported inputs: {}", describe_unsupported(.0))]
    Unsupported(Vec<UnsupportedInput>),

    /// An advisory record could not be used.
    #[error("advisory {id}: {message}")]
    Advisory {
        /// The advisory id, or the file name when the id is unreadable.
        id: String,
        /// What was wrong.
        message: String,
    },

    /// The caller passed a value that cannot be used.
    #[error("{0}")]
    InvalidInput(String),

    /// The store holds something the engine did not expect.
    #[error("unexpected store contents: {0}")]
    Corrupt(String),
}

fn describe_unsupported(inputs: &[UnsupportedInput]) -> String {
    inputs
        .iter()
        .map(|input| input.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

impl Error {
    pub(crate) fn io(
        action: &'static str,
        path: impl Into<PathBuf>,
        source: std::io::Error,
    ) -> Self {
        Error::Io {
            action,
            path: path.into(),
            source,
        }
    }

    pub(crate) fn parse(
        kind: &'static str,
        path: impl Into<PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Error::Parse {
            kind,
            path: path.into(),
            message: message.into(),
        }
    }
}

impl From<relate::Error> for Error {
    fn from(error: relate::Error) -> Self {
        match &error {
            relate::Error::Locked(path) => Error::StoreLocked(path.clone()),
            _ => Error::Store {
                gql_status: relate::error_info(&error).gql_status.to_string(),
                message: error.to_string(),
            },
        }
    }
}
