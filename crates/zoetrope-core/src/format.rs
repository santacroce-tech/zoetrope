//! Versioned project file format. See `docs/FORMAT.md`.
//!
//! A file is a JSON envelope `{ "format", "schemaVersion", "project" }`.
//! Loading parses to an untyped JSON value first, runs the migration chain
//! from the file's version up to `SCHEMA_VERSION`, then deserializes and
//! validates. Files from newer versions are rejected rather than guessed at.

use crate::error::{Error, Result};
use crate::model::Project;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FORMAT_ID: &str = "zoetrope-project";
pub const SCHEMA_VERSION: u32 = 2;

/// One migration step: upgrades a whole envelope from version N to N+1.
/// It may assume `schemaVersion == N`; the runner rewrites the version field.
pub type Migration = fn(Value) -> Result<Value>;

/// `MIGRATIONS[i]` upgrades schema version `i + 1` to `i + 2`.
/// Append here whenever `SCHEMA_VERSION` is bumped.
const MIGRATIONS: &[Migration] = &[v1_to_v2];

/// v1 → v2 (Phase 3): strokes changed from `{ width, color }` to
/// `{ width, paint, cap, join, miterLimit, … }`. Old strokes become solid
/// paints pinned to the caps/joins Canvas2D used before (butt / miter / 10),
/// so v1 files render exactly as they did.
fn v1_to_v2(mut v: Value) -> Result<Value> {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(map) => {
                if let Some(Value::Object(stroke)) = map.get_mut("stroke") {
                    if let Some(color) = stroke.remove("color") {
                        stroke.insert("paint".into(), serde_json::json!({ "type": "solid", "color": color }));
                        stroke.insert("cap".into(), "butt".into());
                        stroke.insert("join".into(), "miter".into());
                        stroke.insert("miterLimit".into(), 10.into());
                    }
                }
                map.values_mut().for_each(walk);
            }
            Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v["project"]);
    Ok(v)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<P> {
    format: String,
    schema_version: u32,
    project: P,
}

pub fn save_to_string(project: &Project) -> String {
    let env = Envelope { format: FORMAT_ID.to_string(), schema_version: SCHEMA_VERSION, project };
    serde_json::to_string_pretty(&env).expect("project serialization cannot fail")
}

pub fn load_from_str(json: &str) -> Result<Project> {
    let value: Value = serde_json::from_str(json)?;
    let value = migrate(value, MIGRATIONS, SCHEMA_VERSION)?;
    let env: Envelope<Project> = serde_json::from_value(value)?;
    env.project.validate()?;
    Ok(env.project)
}

/// Checks the envelope and upgrades it to `target` using `steps`.
pub fn migrate(mut value: Value, steps: &[Migration], target: u32) -> Result<Value> {
    if value.get("format").and_then(Value::as_str) != Some(FORMAT_ID) {
        return Err(Error::Format(format!("missing or wrong \"format\" (expected {FORMAT_ID:?})")));
    }
    let found = value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Format("missing \"schemaVersion\"".into()))?;
    if found == 0 || found > target as u64 {
        return Err(Error::UnsupportedVersion { found, supported: target });
    }
    for v in found as u32..target {
        let step = steps
            .get(v as usize - 1)
            .ok_or_else(|| Error::Format(format!("no migration from schema {v}")))?;
        value = step(value)?;
        value["schemaVersion"] = Value::from(v + 1);
    }
    Ok(value)
}
