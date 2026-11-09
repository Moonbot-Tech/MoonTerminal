//! Exact strategy reads and schema-validated edits; formatting never participates in a write.

use anyhow::{Context, Result, bail, ensure};
use moonproto::{
    FieldValue, MoonClient, StrategyEditor, StrategyFieldType, StrategyKind, StrategySchema,
    StrategySnapshot,
};
use serde_json::{Value, json};

use super::Action;

/// Preserve every numeric bit; 64-bit integers use strings to survive JavaScript MCP clients.
fn value(field: &FieldValue) -> Value {
    match field {
        FieldValue::Bool(v) => json!(v),
        FieldValue::Int32(v) => json!(v),
        FieldValue::Int64(v) => json!(v.to_string()),
        FieldValue::Double(v) => json!(v),
        FieldValue::String(v) => json!(v),
        FieldValue::Byte(v) => json!(v),
        FieldValue::Word(v) => json!(v),
        FieldValue::UInt32(v) => json!(v),
        FieldValue::UInt64(v) => json!(v.to_string()),
        FieldValue::Single(v) => json!(v),
    }
}

/// An unrounded strategy with revision and identity distinct from its editable fields.
pub(super) fn row(strategy: &StrategySnapshot) -> Value {
    let fields: serde_json::Map<_, _> = strategy
        .fields
        .iter()
        .map(|(k, v)| (k.to_string(), value(v)))
        .collect();
    json!({"id":strategy.strategy_id.to_string(),"version":strategy.strategy_ver,"last_date":strategy.last_date,
        "kind":strategy.kind().ordinal(),"checked":strategy.checked,"path":strategy.path,"fields":fields})
}

/// Read a bounded schema page, including live dynamic picklists omitted by the UI projection.
pub(super) fn schema(
    client: &MoonClient,
    kind: Option<u8>,
    name: Option<&str>,
    offset: usize,
) -> Result<Value> {
    let snapshot = client.snapshot().context("no live snapshot")?;
    let schema = snapshot
        .strats()
        .strategy_schema()
        .context("strategy schema unavailable")?;
    let fields: Vec<_> = schema.fields.iter().filter(|f| kind.is_none_or(|k| f.visible_for_strategy_kind(StrategyKind::from_ordinal(k))) && name.is_none_or(|n| f.name.eq_ignore_ascii_case(n))).skip(offset).take(50).map(|f| json!({
        "name":f.name,"type":f.type_id.name(),"default":f.default_value.as_ref().map(value),
        "visible_kinds":f.visible_strategy_kinds().map(|k| k.ordinal()).collect::<Vec<_>>(),
        "static_choices":f.static_picklist,"dynamic_choices":f.dynamic_picklist.as_ref().map(|p| p.values_from_snapshots(snapshot.strategy_snapshots())),
        "layout":format!("{:?}",f.layout)
    })).collect();
    Ok(
        json!({"format_version":schema.format_version,"kinds":schema.kinds.iter().map(|k| json!({"id":k.ordinal(),"name":k.name})).collect::<Vec<_>>(),
        "next_offset":offset+fields.len(),"fields":fields,"semantics":"Schema supplies types and visibility. Use MoonDoc for units, dependencies and trading meaning. Int64/UInt64 values are decimal strings."}),
    )
}

/// Read retained core state; unresolved local desired values are not passed off as applied values.
pub(super) fn read(client: &MoonClient, id: Option<u64>, offset: usize) -> Result<Value> {
    let snapshot = client.snapshot().context("no live snapshot")?;
    let rows: Vec<_> = snapshot.strategy_snapshots().filter(|s| id.is_none_or(|id| s.strategy_id == id)).skip(offset).take(if id.is_some() { 1 } else { 50 }).map(|s| {
        let mut result = if id.is_some() { row(s) } else { json!({"id":s.strategy_id.to_string(),"name":s.fields.get("StrategyName").map(value),"kind":s.kind().ordinal(),"checked":s.checked,"path":s.path}) };
        result["pending_edit"] = json!(snapshot.strategy_edit(s.strategy_id).is_some());
        result
    }).collect();
    ensure!(id.is_none() || !rows.is_empty(), "strategy not found");
    Ok(
        json!({"source":"ActiveLib core snapshot","next_offset":offset+rows.len(),"strategies":rows}),
    )
}

/// A preview refuses to compete with an existing local edit of the same strategy.
pub(super) fn preview(client: &MoonClient, action: &Action) -> Result<Value> {
    let snapshot = client.snapshot().context("no live snapshot")?;
    let schema = snapshot
        .strats()
        .strategy_schema()
        .context("strategy schema unavailable")?;
    match action {
        Action::StrategyEdit { id, .. } | Action::StrategyDelete { id } => ensure!(
            snapshot.strategy_edit(*id).is_none(),
            "strategy already has a pending edit; observe its result first"
        ),
        Action::StrategyCreate { .. } => ensure!(
            snapshot.strats().strategy_edits().next().is_none(),
            "wait for pending strategy edits before creating a strategy"
        ),
        _ => {}
    }
    let mut full = snapshot.strategy_snapshot_vec();
    transform(&mut full, schema, action, None)
}

/// Rebuild on the effective full set (including pending edits), checking the complete preview first.
pub(crate) fn apply(
    full: &mut Vec<StrategySnapshot>,
    schema: Option<&StrategySchema>,
    action: &Action,
    expected: &Value,
    now: u64,
) -> Result<Value> {
    let schema = schema.context("strategy schema unavailable")?;
    // Work on a copy until every field validates and the previous snapshot still matches.
    let mut next = full.clone();
    let mut preview = transform(&mut next, schema, action, Some(now))?;
    ensure!(
        preview["before"] == expected["before"] && preview["after"] == expected["after"],
        "strategy changed since preview; read and preview again"
    );
    if let Some(sent) = next
        .iter()
        .find(|strategy| Some(strategy.strategy_id) == super::ids::parse(&preview["id"]).ok())
    {
        preview["sent"] = row(sent);
    }
    *full = next;
    Ok(preview)
}

/// Build one complete, validated replacement without overwriting untouched or future fields.
fn transform(
    full: &mut Vec<StrategySnapshot>,
    schema: &StrategySchema,
    action: &Action,
    now: Option<u64>,
) -> Result<Value> {
    let (before, mut changed, apply_orders) = match action {
        Action::StrategyEdit {
            id,
            fields,
            checked,
            path,
            apply_to_orders,
        } => {
            let old = full
                .iter()
                .find(|s| s.strategy_id == *id)
                .context("strategy not found")?;
            // Only a rename is judged: an edit that keeps its name must stay possible even on a
            // pair that already shares one, or the duplicate could never be fixed from here.
            if let Some(next) = fields.get("StrategyName") {
                let next = next.as_str().context("StrategyName must be a string")?;
                if old.strategy_name().map(str::trim) != Some(next.trim()) {
                    ensure_name_free(full, Some(*id), next)?;
                }
            }
            let mut base = old.clone();
            if let Some(signal) = fields.get("SignalType") {
                let name = signal.as_str().context("SignalType must be a string")?;
                let kind = schema
                    .kinds
                    .iter()
                    .find(|k| k.name == name)
                    .context("unknown strategy kind")?
                    .kind();
                base = StrategySnapshot::new(
                    base.strategy_id,
                    base.strategy_ver,
                    base.last_date,
                    base.checked,
                    kind,
                    base.path,
                    base.fields,
                );
            }
            let mut editor = StrategyEditor::from_snapshot(schema, &base)?;
            edit_fields(&mut editor, schema, fields)?;
            if let Some(checked) = checked {
                editor.set_checked(*checked);
            }
            if let Some(path) = path {
                validate_path(path)?;
                editor.set_path(path.clone());
            }
            (row(old), editor.into_snapshot(), *apply_to_orders)
        }
        Action::StrategyCreate {
            strategy_kind,
            name,
            path,
            fields,
        } => {
            validate_path(path)?;
            ensure!(
                !name.trim().is_empty() && name.len() <= 128,
                "strategy name must contain 1..128 bytes"
            );
            ensure_name_free(full, None, name)?;
            let id = full
                .iter()
                .map(|s| s.strategy_id)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .context("strategy ID exhausted")?;
            let kind = StrategyKind::from_ordinal(*strategy_kind);
            let mut editor = StrategyEditor::new(schema, id, kind, path.clone())?;
            edit_fields(&mut editor, schema, fields)?;
            editor.set_string("StrategyName", name.clone())?;
            let kind_name = schema
                .kind_name_for_strategy_kind(kind)
                .context("unknown strategy kind")?;
            editor.set_string("SignalType", kind_name)?;
            editor.set_checked(false);
            let mut made = editor.into_snapshot();
            // Preview identity is stable; the wall clock is applied only at submission.
            made.last_date = 0;
            // The editor stamps format 1, which the core would convert as an old strategy file —
            // a MoonShot would lose its TriggerKeyBuy (#1286). Stamped like every other create.
            made.strategy_ver = crate::feed::live::revision_stamp(full);
            (json!({"next_id":id.to_string()}), made, false)
        }
        Action::StrategyDelete { id } => {
            let old = full
                .iter()
                .find(|s| s.strategy_id == *id)
                .context("strategy not found")?;
            ensure!(
                !old.checked,
                "uncheck the strategy and observe confirmation before deleting it"
            );
            return Ok(json!({"before":row(old),"after":null,"id":id.to_string(),"delete":true}));
        }
        _ => bail!("not a strategy action"),
    };
    let after = row(&changed);
    if let Some(now) = now {
        changed.last_date = now.max(changed.last_date.saturating_add(1));
    }
    let id = changed.strategy_id;
    if let Some(existing) = full.iter_mut().find(|s| s.strategy_id == id) {
        *existing = changed;
    } else {
        full.push(changed);
    }
    Ok(
        json!({"before":before,"after":after,"id":id.to_string(),"apply_to_orders":apply_orders,
        "limits":"No server-side compare-and-swap. An external edit after submission can still race. Settings confirmation does not confirm BUY replacement."}),
    )
}

/// Refuse a strategy name another strategy on this core already carries.
///
/// A rule of the Station's automation path only, not of the core: the core accepts two strategies
/// of one name, but `UseHookStrategy` names its MoonHook BY NAME, and with a Manual and a hook
/// both called the same the core priced orders off the Manual's parameters instead of the hook's.
/// A person may still make such a pair in MoonBot; the Station's AI client may not.
///
/// Matching trims surrounding whitespace and ignores case. Whether the core's own name lookup
/// folds case is not known, so the guard takes the stricter reading: refusing `fast - 1` beside
/// `Fast - 1` costs an AI client one other name, while letting it through could rebuild the very
/// collision this exists to stop. `own` is the strategy being renamed, which never collides with
/// itself. An empty name collides with nothing; the callers bound the name themselves.
fn ensure_name_free(full: &[StrategySnapshot], own: Option<u64>, name: &str) -> Result<()> {
    let wanted = name.trim();
    if wanted.is_empty() {
        return Ok(());
    }
    let folded = wanted.to_lowercase();
    if let Some(other) = full.iter().find(|s| {
        Some(s.strategy_id) != own
            && s.strategy_name()
                .is_some_and(|n| n.trim().to_lowercase() == folded)
    }) {
        bail!(
            "strategy name {wanted:?} is already used by strategy {} on this core; pick another \
             name (UseHookStrategy and other references resolve strategies by name)",
            other.strategy_id
        );
    }
    Ok(())
}

/// A folder is a logical strategy path, never a filesystem path or implicit delete operation.
fn validate_path(path: &str) -> Result<()> {
    ensure!(
        path.len() <= 512 && !path.chars().any(char::is_control),
        "invalid strategy folder path"
    );
    Ok(())
}

/// Let ActiveLib validate existence, kind visibility, exact type and numeric representation.
fn edit_fields(
    editor: &mut StrategyEditor<'_>,
    schema: &StrategySchema,
    fields: &std::collections::BTreeMap<String, Value>,
) -> Result<()> {
    ensure!(fields.len() <= 256, "too many fields in one edit");
    for (name, input) in fields {
        let field = schema
            .field(name)
            .with_context(|| format!("unknown field {name}"))?;
        match field.type_id {
            StrategyFieldType::Bool => {
                editor.set_bool(name, input.as_bool().context("expected boolean")?)?
            }
            StrategyFieldType::String => {
                let text = input.as_str().context("expected string")?;
                ensure!(text.len() <= 16_384, "strategy string too long");
                editor.set_string(name, text)?;
            }
            StrategyFieldType::Int64 => {
                let integer = if let Some(text) = input.as_str() {
                    text.parse()?
                } else {
                    input.as_i64().context("expected exact Int64")?
                };
                editor.set_value(name, FieldValue::Int64(integer))?;
            }
            StrategyFieldType::UInt64 => {
                let integer = if let Some(text) = input.as_str() {
                    text.parse()?
                } else {
                    input.as_u64().context("expected exact UInt64")?
                };
                editor.set_value(name, FieldValue::UInt64(integer))?;
            }
            _ => editor.set_number(name, input.as_f64().context("expected number")?)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
