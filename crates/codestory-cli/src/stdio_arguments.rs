//! Validation of `tools/call` arguments against the generated MCP catalog.
//!
//! The catalog published by `tools/list` is the only declaration of the tool
//! argument contract. This module interprets that published JSON directly, so
//! an advertised type, enum, bound, required property, `additionalProperties`
//! rule, or `oneOf` selector is enforced by construction instead of by a
//! parallel hand-written rule set that can drift away from it.
//!
//! The interpreter understands a closed subset of JSON Schema — exactly the
//! keywords the catalog emits. `catalog_keywords_stay_within_the_validated_subset`
//! fails as soon as the catalog grows a keyword this module would silently
//! ignore, which is the anti-drift fence: a new advertised constraint cannot
//! ship unenforced.

use serde_json::{Map, Value};

/// Selector every tool schema declares and every session binder owns.
///
/// The catalog marks `project` required because the multi-project server needs
/// it, but a single-project session legitimately omits it and the binder
/// answers with the richer `project_required` tool error. Presence is therefore
/// the binder's call; the declared type and length still apply here.
const SESSION_OWNED_ARGUMENT: &str = "project";

/// JSON Schema keywords the catalog is allowed to emit.
///
/// Anything outside this set would be advertised to callers but unenforced, so
/// the catalog test rejects it until this module learns the keyword.
#[cfg(test)]
pub(crate) const VALIDATED_KEYWORDS: &[&str] = &[
    "additionalProperties",
    "allOf",
    "anyOf",
    "default",
    "description",
    "enum",
    "items",
    "maxItems",
    "maxLength",
    "maximum",
    "minItems",
    "minLength",
    "minimum",
    "not",
    "oneOf",
    "properties",
    "required",
    "type",
];

/// One rejected argument, carrying a machine code and a JSON pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArgumentViolation {
    code: &'static str,
    pointer: String,
    message: String,
}

impl ArgumentViolation {
    fn new(code: &'static str, pointer: &str, message: impl Into<String>) -> Self {
        Self {
            code,
            pointer: pointer.to_string(),
            message: message.into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn code(&self) -> &'static str {
        self.code
    }

    #[cfg(test)]
    pub(crate) fn pointer(&self) -> &str {
        &self.pointer
    }

    pub(crate) fn to_json(&self) -> Value {
        serde_json::json!({
            "code": self.code,
            "pointer": self.pointer,
            "message": self.message,
        })
    }
}

/// Validate `arguments` for `tool` against the published catalog declaration.
///
/// Unknown tools resolve to `Ok`; the dispatcher rejects those before reaching
/// argument validation and this module must not invent a second answer for the
/// same condition.
pub(crate) fn validate_tool_arguments(
    tool: &str,
    arguments: Option<&Value>,
) -> Result<(), Vec<ArgumentViolation>> {
    let Some(schema) = crate::stdio_catalog::tool_input_schema(tool) else {
        return Ok(());
    };
    // The dispatcher deliberately admits absent and null arguments; both mean
    // "no arguments supplied", which the schema still has to accept or reject.
    let empty = Value::Object(Map::new());
    let arguments = match arguments {
        None | Some(Value::Null) => &empty,
        Some(value) => value,
    };
    let mut violations = Vec::new();
    validate_value(schema, arguments, "/arguments", &mut violations);
    let session_owned = format!("/arguments/{SESSION_OWNED_ARGUMENT}");
    violations.retain(|violation| {
        !(violation.code == "missing_required" && violation.pointer == session_owned)
    });
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

fn validate_value(schema: &Value, value: &Value, pointer: &str, out: &mut Vec<ArgumentViolation>) {
    let Some(schema) = schema.as_object() else {
        return;
    };
    if let Some(declared) = schema.get("type") {
        if !type_matches(declared, value) {
            out.push(ArgumentViolation::new(
                "invalid_type",
                pointer,
                format!(
                    "expected type {}, received {}",
                    render_type(declared),
                    json_type_name(value)
                ),
            ));
            return;
        }
        // A declared-nullable member carries no further constraints when null.
        if value.is_null() {
            return;
        }
    }
    validate_enum(schema, value, pointer, out);
    validate_number_bounds(schema, value, pointer, out);
    validate_string_length(schema, value, pointer, out);
    validate_array(schema, value, pointer, out);
    validate_object(schema, value, pointer, out);
    validate_combinators(schema, value, pointer, out);
}

fn validate_enum(
    schema: &Map<String, Value>,
    value: &Value,
    pointer: &str,
    out: &mut Vec<ArgumentViolation>,
) {
    let Some(allowed) = schema.get("enum").and_then(Value::as_array) else {
        return;
    };
    if !allowed.contains(value) {
        out.push(ArgumentViolation::new(
            "invalid_enum_value",
            pointer,
            format!(
                "expected one of {}",
                allowed
                    .iter()
                    .map(render_literal)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
}

fn validate_number_bounds(
    schema: &Map<String, Value>,
    value: &Value,
    pointer: &str,
    out: &mut Vec<ArgumentViolation>,
) {
    let Some(number) = value.as_f64() else {
        return;
    };
    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
        && number < minimum
    {
        out.push(ArgumentViolation::new(
            "below_minimum",
            pointer,
            format!("expected a value of at least {}", render_bound(minimum)),
        ));
    }
    if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64)
        && number > maximum
    {
        out.push(ArgumentViolation::new(
            "above_maximum",
            pointer,
            format!("expected a value of at most {}", render_bound(maximum)),
        ));
    }
}

fn validate_string_length(
    schema: &Map<String, Value>,
    value: &Value,
    pointer: &str,
    out: &mut Vec<ArgumentViolation>,
) {
    let Some(text) = value.as_str() else {
        return;
    };
    let length = text.chars().count() as u64;
    if let Some(minimum) = schema.get("minLength").and_then(Value::as_u64)
        && length < minimum
    {
        out.push(ArgumentViolation::new(
            "below_min_length",
            pointer,
            format!("expected at least {minimum} character(s)"),
        ));
    }
    if let Some(maximum) = schema.get("maxLength").and_then(Value::as_u64)
        && length > maximum
    {
        out.push(ArgumentViolation::new(
            "above_max_length",
            pointer,
            format!("expected at most {maximum} character(s)"),
        ));
    }
}

fn validate_array(
    schema: &Map<String, Value>,
    value: &Value,
    pointer: &str,
    out: &mut Vec<ArgumentViolation>,
) {
    let Some(items) = value.as_array() else {
        return;
    };
    let count = items.len() as u64;
    if let Some(minimum) = schema.get("minItems").and_then(Value::as_u64)
        && count < minimum
    {
        out.push(ArgumentViolation::new(
            "below_min_items",
            pointer,
            format!("expected at least {minimum} item(s)"),
        ));
    }
    if let Some(maximum) = schema.get("maxItems").and_then(Value::as_u64)
        && count > maximum
    {
        out.push(ArgumentViolation::new(
            "above_max_items",
            pointer,
            format!("expected at most {maximum} item(s)"),
        ));
    }
    let Some(item_schema) = schema.get("items") else {
        return;
    };
    for (index, item) in items.iter().enumerate() {
        validate_value(item_schema, item, &format!("{pointer}/{index}"), out);
    }
}

fn validate_object(
    schema: &Map<String, Value>,
    value: &Value,
    pointer: &str,
    out: &mut Vec<ArgumentViolation>,
) {
    let Some(members) = value.as_object() else {
        return;
    };
    let properties = schema.get("properties").and_then(Value::as_object);
    if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
        let declared = properties;
        for name in members.keys() {
            if !declared.is_some_and(|declared| declared.contains_key(name)) {
                out.push(ArgumentViolation::new(
                    "unknown_property",
                    &format!("{pointer}/{name}"),
                    "property is not declared by the published tool schema",
                ));
            }
        }
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !members.contains_key(name) {
                out.push(ArgumentViolation::new(
                    "missing_required",
                    &format!("{pointer}/{name}"),
                    "required property is missing",
                ));
            }
        }
    }
    let Some(properties) = properties else {
        return;
    };
    for (name, member) in members {
        if let Some(property_schema) = properties.get(name) {
            validate_value(property_schema, member, &format!("{pointer}/{name}"), out);
        }
    }
}

fn validate_combinators(
    schema: &Map<String, Value>,
    value: &Value,
    pointer: &str,
    out: &mut Vec<ArgumentViolation>,
) {
    if let Some(variants) = schema.get("anyOf").and_then(Value::as_array)
        && !variants.iter().any(|variant| accepts(variant, value))
    {
        out.push(ArgumentViolation::new(
            "unsatisfied_any_of",
            pointer,
            format!("expected at least one of {}", render_variants(variants)),
        ));
    }
    if let Some(variants) = schema.get("oneOf").and_then(Value::as_array) {
        let matched = variants
            .iter()
            .filter(|variant| accepts(variant, value))
            .count();
        if matched != 1 {
            out.push(ArgumentViolation::new(
                "invalid_selector",
                pointer,
                format!(
                    "expected exactly one of {}, matched {matched}",
                    render_variants(variants)
                ),
            ));
        }
    }
    if let Some(constraints) = schema.get("allOf").and_then(Value::as_array)
        && let Some(failed) = constraints
            .iter()
            .find(|constraint| !accepts(constraint, value))
    {
        out.push(combined_constraint_violation(
            constraints.len(),
            failed,
            pointer,
        ));
    }
    if let Some(forbidden) = schema.get("not")
        && accepts(forbidden, value)
    {
        out.push(ArgumentViolation::new(
            "forbidden_combination",
            pointer,
            "value matches a combination the tool schema forbids",
        ));
    }
}

/// Render the catalog's combined-item ceiling, which it encodes as one `not`
/// per admissible split of the shared budget.
fn combined_constraint_violation(
    constraint_count: usize,
    failed: &Value,
    pointer: &str,
) -> ArgumentViolation {
    let names = failed
        .pointer("/not/required")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(" and ")
        });
    match names {
        Some(names) if !names.is_empty() => ArgumentViolation::new(
            "combined_item_limit",
            pointer,
            format!("{names} may hold at most {constraint_count} item(s) together"),
        ),
        _ => ArgumentViolation::new(
            "unsatisfied_all_of",
            pointer,
            "value violates a declared combined constraint",
        ),
    }
}

fn accepts(schema: &Value, value: &Value) -> bool {
    let mut violations = Vec::new();
    validate_value(schema, value, "", &mut violations);
    violations.is_empty()
}

fn type_matches(declared: &Value, value: &Value) -> bool {
    match declared {
        Value::String(name) => matches_type_name(name, value),
        Value::Array(names) => names
            .iter()
            .filter_map(Value::as_str)
            .any(|name| matches_type_name(name, value)),
        _ => true,
    }
}

fn matches_type_name(name: &str, value: &Value) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "null" => value.is_null(),
        _ => false,
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) => {
            if number.is_i64() || number.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn render_type(declared: &Value) -> String {
    match declared {
        Value::String(name) => name.clone(),
        Value::Array(names) => names
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" or "),
        other => other.to_string(),
    }
}

fn render_literal(value: &Value) -> String {
    value.as_str().map_or_else(
        || value.to_string(),
        |text| {
            let mut rendered = String::with_capacity(text.len() + 2);
            rendered.push('`');
            rendered.push_str(text);
            rendered.push('`');
            rendered
        },
    )
}

fn render_bound(bound: f64) -> String {
    if bound.fract() == 0.0 {
        format!("{bound:.0}")
    } else {
        bound.to_string()
    }
}

/// Name the declared alternatives by the properties they select on, so a
/// caller reading the typed data learns which selector to send.
fn render_variants(variants: &[Value]) -> String {
    variants
        .iter()
        .map(|variant| {
            variant
                .get("required")
                .and_then(Value::as_array)
                .map(|names| {
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(" + ")
                })
                .filter(|rendered| !rendered.is_empty())
                .unwrap_or_else(|| "the declared variant".to_string())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn codes(tool: &str, arguments: Value) -> Vec<&'static str> {
        match validate_tool_arguments(tool, Some(&arguments)) {
            Ok(()) => Vec::new(),
            Err(violations) => violations
                .iter()
                .map(super::ArgumentViolation::code)
                .collect(),
        }
    }

    fn pointers(tool: &str, arguments: Value) -> Vec<String> {
        match validate_tool_arguments(tool, Some(&arguments)) {
            Ok(()) => Vec::new(),
            Err(violations) => violations
                .iter()
                .map(|violation| violation.pointer().to_string())
                .collect(),
        }
    }

    fn collect_keywords(schema: &Value, found: &mut std::collections::BTreeSet<String>) {
        match schema {
            Value::Object(members) => {
                let is_schema = members.contains_key("type")
                    || members.contains_key("properties")
                    || members.contains_key("required")
                    || members.contains_key("oneOf")
                    || members.contains_key("anyOf")
                    || members.contains_key("allOf")
                    || members.contains_key("not")
                    || members.contains_key("items")
                    || members.contains_key("enum");
                for (key, value) in members {
                    if is_schema {
                        found.insert(key.clone());
                    }
                    if is_schema && key == "properties" {
                        if let Some(properties) = value.as_object() {
                            for property in properties.values() {
                                collect_keywords(property, found);
                            }
                        }
                        continue;
                    }
                    collect_keywords(value, found);
                }
            }
            Value::Array(items) => {
                for item in items {
                    collect_keywords(item, found);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn catalog_keywords_stay_within_the_validated_subset() {
        let mut found = std::collections::BTreeSet::new();
        for tool in crate::stdio_catalog::tool_names() {
            let schema = crate::stdio_catalog::tool_input_schema(tool).expect("published schema");
            collect_keywords(schema, &mut found);
        }
        let unsupported = found
            .iter()
            .filter(|keyword| !VALIDATED_KEYWORDS.contains(&keyword.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            unsupported.is_empty(),
            "the catalog advertises keywords this validator ignores: {unsupported:?}"
        );
    }

    #[test]
    fn every_tool_declares_a_published_input_schema() {
        for tool in crate::stdio_catalog::tool_names() {
            let schema = crate::stdio_catalog::tool_input_schema(tool).expect("published schema");
            assert_eq!(
                schema.get("additionalProperties"),
                Some(&json!(false)),
                "{tool} must deny undeclared arguments"
            );
        }
    }

    #[test]
    fn valid_arguments_pass_every_declared_constraint() {
        for (tool, arguments) in [
            ("status", json!({"project": "/repo"})),
            (
                "search",
                json!({"project": "/repo", "query": "router", "repo_text": "auto", "limit": 10}),
            ),
            ("ground", json!({"project": "/repo", "budget": "strict"})),
            (
                "packet",
                json!({
                    "project": "/repo",
                    "question": "how does routing work",
                    "budget": "compact",
                    "task_class": null,
                    "probes": [{"kind": "free_query", "query": "router"}],
                    "extra_probes": ["router"],
                    "latency_budget_ms": 5000,
                    "parent_packet_id": "packet-1",
                    "option_ids": ["bounded_source_read:src%2Funread.rs"],
                    "core_generation_id": "core-1"
                }),
            ),
            (
                "affected",
                json!({"project": "/repo", "paths": ["src/lib.rs"], "depth": 2}),
            ),
            ("symbol", json!({"project": "/repo", "query": "run"})),
            (
                "shortest_path",
                json!({"project": "/repo", "from_id": "1", "to_id": "2"}),
            ),
            (
                "snippet",
                json!({"project": "/repo", "id": "7", "scope": "function_body", "context": 4}),
            ),
            (
                "context",
                json!({"project": "/repo", "bookmark": "bm-1", "include_evidence": false}),
            ),
        ] {
            assert_eq!(
                validate_tool_arguments(tool, Some(&arguments)),
                Ok(()),
                "{tool} rejected its own advertised argument shape"
            );
        }
    }

    #[test]
    fn packet_schema_rejects_undocumented_cli_flags() {
        for (flag, value) in [
            ("file", json!("src/lib.rs")),
            ("mode", json!("compact")),
            ("max_snippet_bytes", json!(2048)),
        ] {
            let mut arguments = json!({
                "project": "/repo",
                "question": "how does routing work"
            });
            arguments
                .as_object_mut()
                .expect("object")
                .insert(flag.to_string(), value);
            assert_eq!(
                codes("packet", arguments),
                vec!["unknown_property"],
                "packet must reject undocumented CLI flag {flag}"
            );
        }
    }

    #[test]
    fn undeclared_properties_are_rejected_instead_of_silently_dropped() {
        assert_eq!(
            codes(
                "search",
                json!({"project": "/repo", "query": "run", "limt": 3})
            ),
            vec!["unknown_property"]
        );
        assert_eq!(
            pointers(
                "search",
                json!({"project": "/repo", "query": "run", "limt": 3})
            ),
            vec!["/arguments/limt".to_string()]
        );
    }

    #[test]
    fn advertised_enums_and_bounds_are_enforced() {
        assert_eq!(
            codes(
                "search",
                json!({"project": "/repo", "query": "run", "repo_text": "yes"})
            ),
            vec!["invalid_enum_value"]
        );
        assert_eq!(
            codes(
                "search",
                json!({"project": "/repo", "query": "run", "limit": 500})
            ),
            vec!["above_maximum"]
        );
        assert_eq!(
            codes(
                "search",
                json!({"project": "/repo", "query": "run", "limit": 0})
            ),
            vec!["below_minimum"]
        );
        assert_eq!(
            codes(
                "search",
                json!({"project": "/repo", "query": "run", "limit": "10"})
            ),
            vec!["invalid_type"]
        );
        assert_eq!(
            codes(
                "search",
                json!({"project": "/repo", "query": "run", "limit": 2.5})
            ),
            vec!["invalid_type"]
        );
        assert_eq!(
            codes("search", json!({"project": "/repo", "query": ""})),
            vec!["below_min_length"]
        );
    }

    #[test]
    fn required_properties_other_than_the_session_selector_are_enforced() {
        assert_eq!(
            codes("search", json!({"project": "/repo"})),
            vec!["missing_required"]
        );
        // The session binder owns `project` presence and answers with the
        // richer project_required tool error.
        assert_eq!(validate_tool_arguments("status", Some(&json!({}))), Ok(()));
        assert_eq!(validate_tool_arguments("status", None), Ok(()));
        assert_eq!(
            codes("status", json!({"project": ""})),
            vec!["below_min_length"]
        );
        assert_eq!(codes("status", json!({"project": 7})), vec!["invalid_type"]);
    }

    #[test]
    fn exactly_one_target_selector_is_accepted() {
        assert_eq!(
            codes(
                "symbol",
                json!({"project": "/repo", "query": "run", "id": "7"})
            ),
            vec!["invalid_selector"]
        );
        assert_eq!(
            codes("symbol", json!({"project": "/repo"})),
            vec!["invalid_selector"]
        );
        assert_eq!(
            codes(
                "context",
                json!({"project": "/repo", "query": "run", "bookmark": "bm-1"})
            ),
            vec!["invalid_selector"]
        );
        assert_eq!(
            codes(
                "affected",
                json!({"project": "/repo", "paths": ["a"], "changed_paths": ["b"]})
            ),
            vec!["invalid_selector"]
        );
    }

    #[test]
    fn tagged_probe_unions_and_shared_budgets_are_enforced() {
        assert_eq!(
            codes(
                "packet",
                json!({
                    "project": "/repo",
                    "question": "why",
                    "probes": [{"kind": "exact_path"}]
                })
            ),
            vec!["invalid_selector"]
        );
        assert_eq!(
            pointers(
                "packet",
                json!({
                    "project": "/repo",
                    "question": "why",
                    "probes": [{"kind": "free_query", "query": "a"}, {"kind": "nope"}]
                })
            ),
            vec!["/arguments/probes/1".to_string()]
        );
        let probes = (0..codestory_contracts::api::PACKET_PROBE_MAX_COUNT - 1)
            .map(|index| json!({"kind": "free_query", "query": format!("probe-{index}")}))
            .collect::<Vec<_>>();
        assert_eq!(
            codes(
                "packet",
                json!({
                    "project": "/repo",
                    "question": "why",
                    "probes": probes,
                    "extra_probes": ["one", "two"]
                })
            ),
            vec!["combined_item_limit"]
        );
    }

    #[test]
    fn nullable_members_accept_null_and_non_nullable_members_do_not() {
        assert_eq!(
            validate_tool_arguments(
                "packet",
                Some(&json!({"project": "/repo", "question": "why", "task_class": null}))
            ),
            Ok(())
        );
        assert_eq!(
            codes(
                "packet",
                json!({"project": "/repo", "question": "why", "budget": null})
            ),
            vec!["invalid_type"]
        );
    }

    #[test]
    fn array_item_constraints_are_enforced() {
        assert_eq!(
            codes("affected", json!({"project": "/repo", "paths": [""]})),
            vec!["below_min_length"]
        );
        assert_eq!(
            codes("affected", json!({"project": "/repo", "paths": []})),
            vec!["below_min_items"]
        );
        assert_eq!(
            codes("affected", json!({"project": "/repo", "paths": [1]})),
            vec!["invalid_type"]
        );
    }

    #[test]
    fn unknown_tools_defer_to_the_dispatcher() {
        assert_eq!(
            validate_tool_arguments("not_a_tool", Some(&json!({"anything": true}))),
            Ok(())
        );
    }
}

/// Argument names that mean the same thing, one group per concept.
///
/// A tool's *output* calls a stable node identifier `node_id`; several tools' *input*
/// schemas call it `id`. An agent that reads `node_id` from a search result and hands it
/// straight back is doing the obvious thing, and the server rejected it as an undeclared
/// property. Across a 54-row benchmark that single mismatch produced 98 `unknown_property`
/// rejections, and the agent -- given an error that named the pointer but not the accepted
/// spelling -- retried the same shape rather than renaming the field.
///
/// `depth` and `max_depth` are the same story between `trail` and `shortest_path`.
///
/// Reconciling here rather than in each schema keeps one published name per concept, so
/// the catalog stays unambiguous while the server accepts its own output vocabulary.
const ARGUMENT_SYNONYMS: &[&[&str]] = &[
    // A hit's identifier. Emitted 1,978 times across a 54-row census as `node_id`.
    &["id", "node_id"],
    // A hit's name. `display_name` appears 2,177 times, `qualified_name` 771, `label` 837 --
    // and every consumer tool declares only `query`, so piping a result into the next call
    // meant renaming the field by hand every time.
    &["query", "display_name", "qualified_name", "label"],
    // A hit's file. `file_path` appears 2,903 times; `snippet` range entries call it `path`.
    &["path", "file_path"],
    &["depth", "max_depth"],
];

/// Rewrite supplied argument names to the spelling this tool's schema declares.
///
/// Only ever renames when exactly one member of a synonym group is declared and the caller
/// used a different member that the schema does not declare; an argument the schema already
/// accepts is never touched, and a collision between two spellings is left alone so
/// validation still reports it.
pub(crate) fn reconcile_argument_synonyms(tool: &str, arguments: &mut Value) {
    let Some(schema) = crate::stdio_catalog::tool_input_schema(tool) else {
        return;
    };
    let Some(declared) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    let Some(supplied) = arguments.as_object_mut() else {
        return;
    };
    for group in ARGUMENT_SYNONYMS {
        let mut accepted = group.iter().filter(|name| declared.contains_key(**name));
        let (Some(canonical), None) = (accepted.next(), accepted.next()) else {
            continue;
        };
        if supplied.contains_key(*canonical) {
            continue;
        }
        let Some(alias) = group
            .iter()
            .find(|name| *name != canonical && supplied.contains_key(**name))
        else {
            continue;
        };
        if let Some(value) = supplied.remove(*alias) {
            supplied.insert((*canonical).to_string(), value);
        }
    }
}

#[cfg(test)]
mod synonym_tests {
    use super::*;
    use serde_json::json;

    /// The server's search results name a stable identifier `node_id`; `symbol` declares
    /// `id`. Handing a result field straight back must work.
    #[test]
    fn node_id_is_accepted_where_the_schema_declares_id() {
        let mut arguments = json!({"project": "/tmp/repo", "node_id": "12345"});
        assert!(
            validate_tool_arguments("symbol", Some(&arguments)).is_err(),
            "precondition: the published schema does not declare node_id"
        );
        reconcile_argument_synonyms("symbol", &mut arguments);
        assert_eq!(arguments["id"], json!("12345"));
        assert!(arguments.get("node_id").is_none());
        assert_eq!(validate_tool_arguments("symbol", Some(&arguments)), Ok(()));
    }

    /// `trail` declares `depth`; `shortest_path` declares `max_depth`. An agent moving
    /// between them should not have to remember which is which.
    #[test]
    fn max_depth_is_accepted_where_the_schema_declares_depth() {
        let mut arguments = json!({"project": "/tmp/repo", "query": "Session", "max_depth": 2});
        reconcile_argument_synonyms("trail", &mut arguments);
        assert_eq!(arguments["depth"], json!(2));
        assert_eq!(validate_tool_arguments("trail", Some(&arguments)), Ok(()));
    }

    /// A spelling the schema already declares is never rewritten.
    #[test]
    fn declared_names_are_left_alone() {
        let mut arguments = json!({"project": "/tmp/repo", "id": "keep-me"});
        reconcile_argument_synonyms("symbol", &mut arguments);
        assert_eq!(arguments["id"], json!("keep-me"));
    }

    /// Supplying both spellings is a real ambiguity; leave it for validation to report
    /// rather than silently picking one.
    #[test]
    fn colliding_spellings_are_not_silently_merged() {
        let mut arguments = json!({"project": "/tmp/repo", "id": "a", "node_id": "b"});
        reconcile_argument_synonyms("symbol", &mut arguments);
        assert_eq!(arguments["id"], json!("a"));
        assert_eq!(arguments["node_id"], json!("b"));
        assert!(validate_tool_arguments("symbol", Some(&arguments)).is_err());
    }
}

#[cfg(test)]
mod source_range_tests {
    use super::*;
    use serde_json::json;

    /// Reading by path is a declared target, so schema validation must accept it.
    #[test]
    fn snippet_accepts_batched_file_ranges() {
        let arguments = json!({
            "project": "/tmp/repo",
            "paths": [
                {"path": "src/a.rs", "start_line": 10, "end_line": 40},
                {"path": "src/b.rs", "start_line": 1, "end_line": 12}
            ]
        });
        assert_eq!(validate_tool_arguments("snippet", Some(&arguments)), Ok(()));
    }

    /// A hit reports `file_path` and a single `line`. Pasting exactly that must work, because
    /// requiring callers to invent an `end_line` is why the batch went unused: 8 of 507
    /// snippet events mentioned `paths` at all.
    #[test]
    fn snippet_accepts_the_line_field_hits_actually_emit() {
        let arguments = json!({
            "project": "/tmp/repo",
            "paths": [{"path": "ChinookDatabase/DataSources/Chinook.sql", "line": 34}]
        });
        assert_eq!(validate_tool_arguments("snippet", Some(&arguments)), Ok(()));
    }

    #[test]
    fn snippet_accepts_top_level_path_and_line_from_a_hit() {
        let arguments = json!({
            "project": "/tmp/repo",
            "path": "src/app.ts",
            "line": 12
        });
        assert_eq!(validate_tool_arguments("snippet", Some(&arguments)), Ok(()));
    }

    #[test]
    fn snippet_accepts_symbol_id_as_an_id_alias() {
        let arguments = json!({
            "project": "/tmp/repo",
            "symbol_id": "42"
        });
        assert_eq!(validate_tool_arguments("snippet", Some(&arguments)), Ok(()));
    }

    #[test]
    fn snippet_accepts_file_path_inside_a_paths_entry() {
        let arguments = json!({
            "project": "/tmp/repo",
            "paths": [{"file_path": "src/app.ts", "line": 12}]
        });
        assert_eq!(validate_tool_arguments("snippet", Some(&arguments)), Ok(()));
    }

    /// A range entry still has to name a file.
    #[test]
    fn snippet_range_entries_require_a_path() {
        let arguments = json!({"project": "/tmp/repo", "paths": [{"line": 10}]});
        assert!(validate_tool_arguments("snippet", Some(&arguments)).is_err());
    }
}

#[cfg(test)]
mod emitted_vocabulary_tests {
    use super::*;
    use serde_json::json;

    /// Every field a hit reports should be pasteable into the tool that consumes it.
    /// Measured over a 54-row census, the consumers accepted none of them: `file_path`
    /// (2,903 occurrences), `display_name` (2,177), `line` (2,110), `node_id` (1,978) and
    /// `qualified_name` (771) were all undeclared, so piping one call into the next meant
    /// renaming fields by hand on every hop.
    #[test]
    fn a_hit_display_name_can_be_pasted_as_a_query() {
        for tool in ["symbol", "trail", "context"] {
            let mut arguments = json!({"project": "/tmp/repo", "display_name": "Session.request"});
            assert!(
                validate_tool_arguments(tool, Some(&arguments)).is_err(),
                "precondition: {tool} does not declare display_name"
            );
            reconcile_argument_synonyms(tool, &mut arguments);
            assert_eq!(arguments["query"], json!("Session.request"), "{tool}");
            assert_eq!(
                validate_tool_arguments(tool, Some(&arguments)),
                Ok(()),
                "{tool}"
            );
        }
    }

    #[test]
    fn a_hit_qualified_name_resolves_to_the_query_selector() {
        let mut arguments =
            json!({"project": "/tmp/repo", "qualified_name": "requests.Session.send"});
        reconcile_argument_synonyms("symbol", &mut arguments);
        assert_eq!(arguments["query"], json!("requests.Session.send"));
        assert_eq!(validate_tool_arguments("symbol", Some(&arguments)), Ok(()));
    }

    /// An explicit `query` still wins; aliases never overwrite a declared name.
    #[test]
    fn an_explicit_query_is_not_overwritten_by_an_alias() {
        let mut arguments =
            json!({"project": "/tmp/repo", "query": "keep", "display_name": "other"});
        reconcile_argument_synonyms("symbol", &mut arguments);
        assert_eq!(arguments["query"], json!("keep"));
    }
}
