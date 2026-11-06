//! Full safe-share reads and exact-field patches use ActiveLib's own catalog and typed values.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use anyhow::{Context, Result, ensure};
use moonproto::shared_config::SharedConfig;
use serde_json::{Value, json};

use crate::session::SessionManager;

impl SessionManager {
    /// Read current values and matching schema entries without projecting through UI settings.
    pub fn automation_settings(
        &self,
        core: u64,
        field: Option<&str>,
        offset: usize,
    ) -> Result<Value> {
        let config = self
            .automation_client(core)?
            .settings()
            .build_shared_config()?;
        read(&config, field, offset)
    }
}

/// Compile the static SDK catalog once; no dependency is inferred from similarly named UI fields.
fn catalog() -> &'static BTreeMap<String, Value> {
    static CATALOG: LazyLock<BTreeMap<String, Value>> = LazyLock::new(|| {
        let schema = SharedConfig::json_schema();
        let mut fields = BTreeMap::new();
        flatten(&schema, &schema, "", &mut fields);
        fields
    });
    &CATALOG
}

/// Follow local schema references and emit only fields that can be read or patched as a unit.
fn flatten(root: &Value, schema: &Value, prefix: &str, fields: &mut BTreeMap<String, Value>) {
    let schema = inline(root, schema);
    if let Some(properties) = schema["properties"].as_object() {
        for (name, schema) in properties {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            flatten(root, schema, &path, fields);
        }
    } else {
        fields.insert(prefix.into(), schema);
    }
}

/// Inline the non-recursive config catalog so a returned field's array item schema is self-contained.
fn inline(root: &Value, schema: &Value) -> Value {
    match schema {
        Value::Object(object) => {
            let mut value = object
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| reference.strip_prefix('#'))
                .and_then(|pointer| root.pointer(pointer))
                .map(|target| inline(root, target))
                .unwrap_or_else(|| json!({}));
            for (name, child) in object
                .iter()
                .filter(|(name, _)| name.as_str() != "$ref" && name.as_str() != "$defs")
            {
                value[name] = inline(root, child);
            }
            value
        }
        Value::Array(values) => {
            Value::Array(values.iter().map(|value| inline(root, value)).collect())
        }
        value => value.clone(),
    }
}

/// Resolve exact Rust catalog paths; arrays are replaced as a whole and are never indexed as objects.
pub(crate) fn field<'a>(values: &'a Value, path: &str) -> Result<&'a Value> {
    path.split('.').try_fold(values, |value, part| {
        value
            .as_object()
            .and_then(|object| object.get(part))
            .with_context(|| format!("unknown setting: {path}"))
    })
}

/// Current values are distinct from schema defaults and retain a source catalog version.
fn read(config: &SharedConfig, selected: Option<&str>, offset: usize) -> Result<Value> {
    let values = config.json_values()?;
    let fields: Vec<_> = catalog()
        .iter()
        .filter(|(path, _)| selected.is_none_or(|wanted| wanted == path.as_str()))
        .skip(offset)
        .take(40)
        .map(|(path, schema)| Ok(json!({"path":path,"value":field(&values,path)?,"schema":schema})))
        .collect::<Result<_>>()?;
    ensure!(
        selected.is_none() || !fields.is_empty(),
        "setting does not exist in the SDK catalog"
    );
    Ok(
        json!({"source":"ActiveLib core safe-share snapshot","core_config_version":config.core_config_version(),
        "fields":fields,"next_offset":offset+fields.len(),"total_fields":catalog().len(),
        "contract":"Types and field paths come from the SDK; MoonDoc supplies units and dependencies. JSON null represents a nonfinite core number, not a value to copy into a patch. Preserve unknown fields. A schema default is not a core value. UI-only preferences do not imply a working headless feature."}),
    )
}

/// Preview only the fields being changed; unrelated updates do not invalidate the operation.
pub(crate) fn preview(config: &SharedConfig, changes: &BTreeMap<String, Value>) -> Result<Value> {
    ensure!(
        !changes.is_empty() && changes.len() <= 64,
        "patch requires 1..64 exact fields"
    );
    let before = config.json_values()?;
    let after = config.patch_json(changes)?.json_values()?;
    let mut old_values = BTreeMap::new();
    let mut new_values = BTreeMap::new();
    for name in changes.keys() {
        ensure!(name.len() <= 256, "setting path is too long");
        old_values.insert(name.clone(), field(&before, name)?.clone());
        new_values.insert(name.clone(), field(&after, name)?.clone());
    }
    Ok(
        json!({"before":old_values,"after":new_values,"core_config_version":config.core_config_version(),
        "limits":"The core imports a full safe-share snapshot, without a server-side CAS or transaction. Station reapplies only these fields to its fresh base. Other terminals can still race after dispatch. Observe every changed field after submission."}),
    )
}

/// Compare the requested fields on a later full echo; compact overlays are not confirmation.
pub(crate) fn matches(config: &SharedConfig, expected: &Value) -> Result<bool> {
    let values = config.json_values()?;
    let expected = expected
        .as_object()
        .context("expected settings must be an object")?;
    for (path, expected) in expected {
        if field(&values, path)? != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
