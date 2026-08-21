use std::io::Write;

use schemars::JsonSchema;

/// Generate a JSON Schema document for `T`.
///
/// This is the machine-readable contract between headless tools and the
/// future Quiver UI: the UI reads schemas to build forms and writes RON
/// configs that the tools consume. The UI never becomes business logic.
pub fn schema_for<T: JsonSchema>() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema serialization cannot fail")
}

/// Write a pretty-printed JSON Schema for `T` to `out`.
pub fn write_schema<T: JsonSchema>(out: &mut impl Write) -> serde_json::Result<()> {
    let doc = schemars::schema_for!(T);
    serde_json::to_writer_pretty(&mut *out, &doc)?;
    writeln!(&mut *out).map_err(serde_json::Error::io)
}
