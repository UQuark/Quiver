use std::path::PathBuf;

/// Errors produced while loading configuration files.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed reading config file {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed parsing RON config {path}: {source}")]
    Parse {
        path: PathBuf,
        // Boxed: ron's SpannedError is large (~128 bytes), keeps the
        // Result small per clippy::result_large_err.
        source: Box<ron::error::SpannedError>,
    },
}

/// One semantic validation failure. `path` is a dotted location inside the
/// config (e.g. `theme.font_size_px`) so tools and the UI can point at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}
