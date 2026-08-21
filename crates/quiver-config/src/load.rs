use std::path::Path;

use serde::de::DeserializeOwned;

use crate::error::ConfigError;

/// Load a config value of type `T` from a RON file at `path`.
///
/// RON is the canonical Quiver config format: verbose and explicit on
/// purpose. Tools consume these files directly; the UI generates them.
pub fn load_from_path<T: DeserializeOwned>(path: &Path) -> Result<T, ConfigError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    ron::from_str(&raw).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source: Box::new(source),
    })
}

/// Parse a config value of type `T` from an in-memory RON string.
/// Useful for tests, `--sample-config` round-trips, and stdin pipelines.
pub fn parse_str<T: DeserializeOwned>(raw: &str) -> Result<T, ron::error::SpannedError> {
    ron::from_str(raw)
}

/// Serialize a config value to a RON string (pretty, multi-line).
pub fn to_string<T: serde::Serialize>(value: &T) -> Result<String, ron::error::Error> {
    ron::ser::to_string_pretty(value, ron::ser::PrettyConfig::default())
}
