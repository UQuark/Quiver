//! Quiver configuration subsystem.
//!
//! Configuration files are a first-class interface in Quiver: headless tools
//! consume verbose, explicit RON configs; the future UI generates them from
//! the JSON Schemas exported by this crate.

pub mod error;
pub mod generate;
pub mod load;
pub mod schema;
pub mod validate;

pub use error::{ConfigError, ValidationIssue};
pub use generate::generate_default;
pub use load::{load_from_path, parse_str, to_string};
pub use schema::{schema_for, write_schema};
pub use validate::{Validate, require};
