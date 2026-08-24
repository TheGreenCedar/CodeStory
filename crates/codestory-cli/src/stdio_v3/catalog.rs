use serde_json::{Map, Value, json};

use super::{V3SurfaceSet, profile::McpRevisionV3};

const VENDOR_SAFETY_KEY: &str = "com.thegreencedar.codestory/safety";
const PROJECTION_ROWS_MAX_V3: usize = 256;
const PROJECTION_REFERENCES_MAX_V3: usize = 256;

pub(crate) fn tools_for_revision_v3(revision: McpRevisionV3) -> Vec<Value> {
    tools_for_surface_v3(revision, V3SurfaceSet::EvidenceOnly)
}

pub(crate) fn tools_for_surface_v3(revision: McpRevisionV3, surface: V3SurfaceSet) -> Vec<Value> {
    let mut sources = crate::stdio_catalog::v3_tool_source_json();
    if surface == V3SurfaceSet::WithProof {
        sources.push(proof_tool_source_v3());
    }
    sources
        .into_iter()
        .map(|source| project_tool_v3(revision, &source))
        .collect()
}

pub(crate) fn proof_output_schema_v3() -> Value {
    json!({
        "type": "object",
        "oneOf": [
            proof_complete_schema_v3(),
            proof_budget_exceeded_schema_v3()
        ]
    })
}

fn proof_common_fields_v3(kind: &str) -> Vec<(&str, Value)> {
    vec![
        ("kind", enum_schema_v3(&[kind])),
        ("schema_version", json!({"type":"integer","enum":[1]})),
        ("domain", enum_schema_v3(&["indexed_source_call_path_v1"])),
        (
            "contract_interpretation",
            enum_schema_v3(&["host_supplied"]),
        ),
        ("guard_version", enum_schema_v3(&["clause_guard_v1"])),
        ("source_text_sha256", sha256_schema_v3()),
        ("contract_digest", sha256_schema_v3()),
        (
            "core_publication",
            closed_object_schema_v3(vec![
                ("project_id", string_schema_v3()),
                ("generation_id", string_schema_v3()),
                ("run_id", string_schema_v3()),
            ]),
        ),
        ("disposition", json!({"type":"object"})),
    ]
}

fn proof_complete_schema_v3() -> Value {
    let mut fields = proof_common_fields_v3("complete");
    fields.extend([
        (
            "identities",
            closed_object_schema_v3(vec![
                (
                    "files",
                    json!({"type":"array","items":proof_file_schema_v3(),"maxItems":65536}),
                ),
                (
                    "symbols",
                    json!({"type":"array","items":proof_symbol_schema_v3(),"maxItems":65536}),
                ),
                (
                    "evidence",
                    json!({"type":"array","items":proof_evidence_schema_v3(),"maxItems":65536}),
                ),
            ]),
        ),
        ("spec", json!({"type":"object"})),
        ("clauses", json!({"type":"array"})),
        (
            "steps",
            json!({"type":"array","items":proof_step_schema_v3(),"maxItems":6}),
        ),
        (
            "receipts",
            json!({"type":"array","items":proof_receipt_schema_v3(),"maxItems":6}),
        ),
    ]);
    closed_object_schema_v3(fields)
}

fn proof_budget_exceeded_schema_v3() -> Value {
    let mut fields = proof_common_fields_v3("budget_exceeded");
    fields.extend([
        ("cap_bytes", json!({"type":"integer","minimum":1})),
        (
            "required_complete_size",
            json!({"type":"integer","minimum":1}),
        ),
    ]);
    closed_object_schema_v3(fields)
}

fn proof_file_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("file_node_id", nullable_schema_v3(string_schema_v3())),
        (
            "project_file_components",
            nullable_schema_v3(json!({"type":"array","items":string_schema_v3()})),
        ),
        ("indexed_sha256", nullable_schema_v3(sha256_schema_v3())),
        ("observed_sha256", nullable_schema_v3(sha256_schema_v3())),
    ])
}

fn proof_symbol_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("node_id", string_schema_v3()),
        ("canonical_id", nullable_schema_v3(string_schema_v3())),
        ("qualified_name", nullable_schema_v3(string_schema_v3())),
        ("file", nullable_schema_v3(unsigned_integer_schema_v3())),
    ])
}

fn proof_evidence_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("fact_id", sha256_schema_v3()),
        ("caller", unsigned_integer_schema_v3()),
        ("target", unsigned_integer_schema_v3()),
        ("edge_id", string_schema_v3()),
        ("callsite_identity", string_schema_v3()),
        (
            "chain",
            json!({"type":"array","items":closed_object_schema_v3(vec![
                ("kind", string_schema_v3()), ("symbols", json!({"type":"array","items":unsigned_integer_schema_v3()})),
            ])}),
        ),
        (
            "provenance",
            closed_object_schema_v3(vec![
                ("producer", string_schema_v3()),
                ("fact_schema_version", json!({"type":"integer","minimum":1})),
                ("algorithm", string_schema_v3()),
                ("language_adapter", string_schema_v3()),
                ("language_adapter_version", string_schema_v3()),
                ("parser_fingerprint", sha256_schema_v3()),
                (
                    "dependency_files",
                    json!({"type":"array","items":unsigned_integer_schema_v3()}),
                ),
                ("evidence_sha256", sha256_schema_v3()),
            ]),
        ),
    ])
}

fn proof_receipt_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("receipt_id", string_schema_v3()),
        ("edge_id", string_schema_v3()),
        ("source", unsigned_integer_schema_v3()),
        ("target", unsigned_integer_schema_v3()),
        ("evidence", unsigned_integer_schema_v3()),
        ("exact_callsite_start_byte", unsigned_integer_schema_v3()),
        ("callsite_identity", string_schema_v3()),
        ("column_or_ordinal", unsigned_integer_schema_v3()),
        (
            "containment",
            closed_object_schema_v3(vec![
                ("file", unsigned_integer_schema_v3()),
                ("owner", unsigned_integer_schema_v3()),
                ("start_line", unsigned_integer_schema_v3()),
                ("end_line", unsigned_integer_schema_v3()),
            ]),
        ),
        (
            "line_window",
            closed_object_schema_v3(vec![
                ("kind", enum_schema_v3(&["indexed_line_v1"])),
                ("file", unsigned_integer_schema_v3()),
                ("anchor_line", unsigned_integer_schema_v3()),
                ("byte_start", unsigned_integer_schema_v3()),
                ("byte_end", unsigned_integer_schema_v3()),
                ("text", string_schema_v3()),
            ]),
        ),
    ])
}

fn proof_step_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("step_index", unsigned_integer_schema_v3()),
        (
            "status",
            enum_schema_v3(&[
                "proven",
                "positive_contradiction",
                "certified_absence",
                "unavailable",
                "unknown",
            ]),
        ),
        ("receipt", nullable_schema_v3(unsigned_integer_schema_v3())),
    ])
}

fn project_tool_v3(revision: McpRevisionV3, source: &Value) -> Value {
    let name = source["name"].as_str().expect("tool source name");
    let activates = source
        .pointer("/safety/activatesProject")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let base_description = source["description"]
        .as_str()
        .expect("tool source description");
    let description = if revision < McpRevisionV3::June2025 {
        format!(
            "{base_description} CodeStory effect: {}.",
            if activates {
                "may activate project-local managed cache, indexing, or network-backed retrieval state"
            } else {
                "observational and does not activate managed state"
            }
        )
    } else {
        base_description.to_string()
    };

    let mut projected = Map::from_iter([
        ("name".to_string(), json!(name)),
        ("description".to_string(), json!(description)),
        ("inputSchema".to_string(), source["inputSchema"].clone()),
    ]);
    match revision {
        McpRevisionV3::November2024 => {}
        McpRevisionV3::March2025 => {
            projected.insert("annotations".to_string(), annotations_v3(activates));
        }
        McpRevisionV3::June2025 | McpRevisionV3::November2025 => {
            projected.insert("title".to_string(), json!(title_v3(name)));
            projected.insert(
                "outputSchema".to_string(),
                output_schema_for_tool_v3(name, source, activates),
            );
            projected.insert("annotations".to_string(), annotations_v3(activates));
            projected.insert(
                "_meta".to_string(),
                json!({
                    VENDOR_SAFETY_KEY: {
                        "effect": if activates { "managed_activation" } else { "read_only" },
                        "sideEffects": activates,
                        "activatesProject": activates,
                        "writesRepository": false,
                        "destructive": false,
                        "idempotent": true,
                        "requiresConfirmation": false,
                        "localOnly": !activates,
                        "openWorld": activates
                    }
                }),
            );
        }
    }
    Value::Object(projected)
}

fn annotations_v3(activates: bool) -> Value {
    let mut annotations = Map::from_iter([
        ("destructiveHint".to_string(), json!(false)),
        ("idempotentHint".to_string(), json!(true)),
        ("openWorldHint".to_string(), json!(activates)),
    ]);
    if !activates {
        annotations.insert("readOnlyHint".to_string(), json!(true));
    }
    Value::Object(annotations)
}

fn output_schema_for_tool_v3(name: &str, source: &Value, activates: bool) -> Value {
    let success = match name {
        "prove_call_path" => proof_output_schema_v3(),
        "packet" => packet_output_schema_v3(),
        "context" => context_output_schema_v3(),
        "search" => search_output_schema_v3(),
        _ => source
            .get("outputSchema")
            .cloned()
            .unwrap_or_else(|| json!({"type":"object"})),
    };
    if activates {
        successful_with_preparing_schema_v3(success)
    } else {
        success
    }
}

fn packet_output_schema_v3() -> Value {
    json!({
        "type": "object",
        "oneOf": [packet_complete_schema_v3(), packet_budget_exceeded_schema_v3()]
    })
}

fn packet_complete_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("kind", enum_schema_v3(&["complete"])),
        ("schema_version", schema_version_v3()),
        ("identity", packet_identity_schema_v3()),
        ("publication", publication_schema_v3()),
        ("status", evidence_availability_schema_v3()),
        ("retrieval", retrieval_state_schema_v3()),
        (
            "evidence",
            bounded_array_schema_v3(packet_evidence_schema_v3(), PROJECTION_ROWS_MAX_V3),
        ),
        (
            "gaps",
            bounded_array_schema_v3(projection_gap_schema_v3(), PROJECTION_ROWS_MAX_V3),
        ),
        ("continuation", nullable_schema_v3(continuation_schema_v3())),
        ("diagnostics", diagnostics_capability_schema_v3()),
    ])
}

fn packet_budget_exceeded_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("kind", enum_schema_v3(&["budget_exceeded"])),
        ("schema_version", schema_version_v3()),
        ("identity", packet_identity_schema_v3()),
        ("publication", publication_schema_v3()),
        ("status", enum_schema_v3(&["unavailable"])),
        ("retrieval", retrieval_state_schema_v3()),
        ("diagnostics", diagnostics_capability_schema_v3()),
        (
            "gaps",
            json!({
                "type":"array",
                "items":closed_object_schema_v3(vec![
                    ("identity", gap_identity_schema_v3()),
                    ("kind", enum_schema_v3(&["output_budget_exceeded"])),
                    ("message", nullable_schema_v3(string_schema_v3())),
                ]),
                "minItems":1,
                "maxItems":1
            }),
        ),
        ("maximum_bytes", unsigned_integer_schema_v3()),
        ("required_complete_bytes", unsigned_integer_schema_v3()),
    ])
}

fn context_output_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("kind", enum_schema_v3(&["complete"])),
        ("schema_version", schema_version_v3()),
        ("identity", packet_identity_schema_v3()),
        ("publication", publication_schema_v3()),
        ("status", evidence_availability_schema_v3()),
        (
            "target",
            closed_object_schema_v3(vec![
                ("path", nullable_schema_v3(string_schema_v3())),
                ("symbol_id", nullable_schema_v3(string_schema_v3())),
            ]),
        ),
        (
            "evidence",
            bounded_array_schema_v3(context_evidence_schema_v3(), PROJECTION_ROWS_MAX_V3),
        ),
        (
            "gaps",
            bounded_array_schema_v3(projection_gap_schema_v3(), PROJECTION_ROWS_MAX_V3),
        ),
        ("continuation", nullable_schema_v3(continuation_schema_v3())),
        ("diagnostics", diagnostics_capability_schema_v3()),
    ])
}

fn search_output_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("kind", enum_schema_v3(&["complete"])),
        ("schema_version", schema_version_v3()),
        ("identity", packet_identity_schema_v3()),
        ("publication", publication_schema_v3()),
        ("status", evidence_availability_schema_v3()),
        (
            "evidence",
            bounded_array_schema_v3(search_evidence_schema_v3(), PROJECTION_ROWS_MAX_V3),
        ),
        (
            "gaps",
            bounded_array_schema_v3(projection_gap_schema_v3(), PROJECTION_ROWS_MAX_V3),
        ),
        ("continuation", nullable_schema_v3(continuation_schema_v3())),
        ("retrieval", retrieval_state_schema_v3()),
        ("diagnostics", diagnostics_capability_schema_v3()),
    ])
}

fn packet_identity_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("packet_id", string_schema_v3()),
        ("request_id", string_schema_v3()),
        ("question_sha256", sha256_schema_v3()),
    ])
}

fn publication_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        (
            "core",
            closed_object_schema_v3(vec![
                ("project_id", string_schema_v3()),
                ("generation_id", string_schema_v3()),
                ("run_id", string_schema_v3()),
            ]),
        ),
        (
            "retrieval",
            nullable_schema_v3(closed_object_schema_v3(vec![
                ("core_generation_id", string_schema_v3()),
                ("core_run_id", string_schema_v3()),
                ("retrieval_generation", string_schema_v3()),
                ("retrieval_input_sha256", sha256_schema_v3()),
                ("semantic_generation", string_schema_v3()),
            ])),
        ),
    ])
}

fn retrieval_state_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        (
            "state",
            enum_schema_v3(&["full", "degraded", "unavailable"]),
        ),
        ("generation_id", nullable_schema_v3(string_schema_v3())),
    ])
}

fn evidence_availability_schema_v3() -> Value {
    enum_schema_v3(&[
        "available",
        "continuation_available",
        "no_useful_evidence",
        "unavailable",
    ])
}

fn evidence_identity_schema_v3() -> Value {
    closed_object_schema_v3(vec![("evidence_id", string_schema_v3())])
}

fn gap_identity_schema_v3() -> Value {
    closed_object_schema_v3(vec![("gap_id", string_schema_v3())])
}

fn packet_evidence_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("identity", evidence_identity_schema_v3()),
        (
            "kind",
            enum_schema_v3(&[
                "exact_source",
                "structural_source",
                "graph_relation",
                "retrieval_excerpt",
            ]),
        ),
        ("path", nullable_schema_v3(string_schema_v3())),
        ("symbol_id", nullable_schema_v3(string_schema_v3())),
        (
            "start_line",
            nullable_schema_v3(unsigned_integer_schema_v3()),
        ),
        ("end_line", nullable_schema_v3(unsigned_integer_schema_v3())),
        ("summary", nullable_schema_v3(string_schema_v3())),
    ])
}

fn context_evidence_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("identity", evidence_identity_schema_v3()),
        ("path", string_schema_v3()),
        ("symbol_id", nullable_schema_v3(string_schema_v3())),
        (
            "start_line",
            nullable_schema_v3(unsigned_integer_schema_v3()),
        ),
        ("end_line", nullable_schema_v3(unsigned_integer_schema_v3())),
        ("excerpt", nullable_schema_v3(string_schema_v3())),
    ])
}

fn search_evidence_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("identity", evidence_identity_schema_v3()),
        ("path", string_schema_v3()),
        ("symbol_id", nullable_schema_v3(string_schema_v3())),
        (
            "start_line",
            nullable_schema_v3(unsigned_integer_schema_v3()),
        ),
        ("end_line", nullable_schema_v3(unsigned_integer_schema_v3())),
        ("excerpt", nullable_schema_v3(string_schema_v3())),
    ])
}

fn projection_gap_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("identity", gap_identity_schema_v3()),
        (
            "kind",
            enum_schema_v3(&[
                "evidence_missing",
                "retrieval_unavailable",
                "source_unavailable",
                "continuation_required",
                "output_budget_exceeded",
            ]),
        ),
        ("message", nullable_schema_v3(string_schema_v3())),
    ])
}

fn continuation_schema_v3() -> Value {
    closed_object_schema_v3(vec![
        ("continuation_id", string_schema_v3()),
        (
            "remaining_rounds",
            json!({"type":"integer","minimum":1,"maximum":65535}),
        ),
        (
            "gap_ids",
            bounded_array_schema_v3(gap_identity_schema_v3(), PROJECTION_REFERENCES_MAX_V3),
        ),
    ])
}

fn diagnostics_capability_schema_v3() -> Value {
    json!({
        "type":"object",
        "oneOf":[
            closed_object_schema_v3(vec![("availability", enum_schema_v3(&["unavailable"]))]),
            closed_object_schema_v3(vec![
                ("availability", enum_schema_v3(&["available"])),
                ("reference", closed_object_schema_v3(vec![
                    ("artifact_id", string_schema_v3()),
                    ("sha256", sha256_schema_v3()),
                    ("byte_length", unsigned_integer_schema_v3()),
                    ("uri", string_schema_v3()),
                    ("wall_expiry_epoch_ms", unsigned_integer_schema_v3()),
                ])),
            ]),
        ]
    })
}

fn closed_object_schema_v3(properties: Vec<(&str, Value)>) -> Value {
    let required = properties
        .iter()
        .map(|(name, _)| Value::String((*name).to_string()))
        .collect::<Vec<_>>();
    let properties = properties
        .into_iter()
        .map(|(name, schema)| (name.to_string(), schema))
        .collect::<Map<_, _>>();
    json!({
        "type":"object",
        "properties":properties,
        "required":required,
        "additionalProperties":false
    })
}

fn bounded_array_schema_v3(items: Value, maximum: usize) -> Value {
    json!({"type":"array","items":items,"maxItems":maximum})
}

fn nullable_schema_v3(mut schema: Value) -> Value {
    let object = schema
        .as_object_mut()
        .expect("v3 nullable schema must be an object");
    let declared = object
        .remove("type")
        .expect("v3 nullable schema must declare its type");
    let mut types = declared
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![declared]);
    types.push(json!("null"));
    object.insert("type".to_string(), Value::Array(types));
    schema
}

fn enum_schema_v3(values: &[&str]) -> Value {
    json!({"type":"string","enum":values})
}

fn string_schema_v3() -> Value {
    json!({"type":"string"})
}

fn sha256_schema_v3() -> Value {
    json!({"type":"string","minLength":64,"maxLength":64})
}

fn schema_version_v3() -> Value {
    json!({"type":"integer","enum":[3]})
}

fn unsigned_integer_schema_v3() -> Value {
    json!({"type":"integer","minimum":0})
}

fn successful_with_preparing_schema_v3(success: Value) -> Value {
    json!({
        "type": "object",
        "oneOf": [
            {
                "allOf": [
                    success,
                    {"not":{"type":"object","properties":{"kind":{"enum":["preparing"]}},"required":["kind"]}}
                ]
            },
            {
                "type": "object",
                "properties": {
                    "kind": {"type":"string","enum":["preparing"]},
                    "state": {"type":"string","enum":["preparing"]},
                    "retry_after_ms": {"type":"integer","minimum":1},
                    "operation": {"type":"object"}
                },
                "required": ["kind", "state", "retry_after_ms", "operation"],
                "additionalProperties": false
            }
        ]
    })
}

fn proof_tool_source_v3() -> Value {
    json!({
        "name": "prove_call_path",
        "description": "Verify one host-translated exact indexed source call-path contract against a pinned publication.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "project": {"type":"string","minLength":1},
                "source_text": {"type":"string","minLength":1},
                "clauses": {"type":"array"},
                "spec": {"type":"object"}
            },
            "required": ["project", "source_text", "clauses", "spec"],
            "additionalProperties": false
        },
        "outputSchema": proof_output_schema_v3(),
        "safety": {
            "effect": "read_only",
            "activatesProject": false,
            "sideEffects": false
        }
    })
}

fn title_v3(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), characters.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn field_names(tool: &Value) -> BTreeSet<&str> {
        tool.as_object()
            .expect("tool object")
            .keys()
            .map(String::as_str)
            .collect()
    }

    fn tool<'a>(tools: &'a [Value], name: &str) -> &'a Value {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("missing v3 tool {name}"))
    }

    #[test]
    fn evidence_only_surface_is_closed_and_never_advertises_proof() {
        for revision in McpRevisionV3::all() {
            let tools = tools_for_surface_v3(*revision, super::super::V3SurfaceSet::EvidenceOnly);
            let names = tools
                .iter()
                .map(|tool| tool["name"].as_str().expect("tool name"))
                .collect::<BTreeSet<_>>();
            assert!(!names.contains("prove_call_path"));
            for required in ["packet", "context", "search"] {
                assert!(names.contains(required), "missing evidence tool {required}");
            }
            assert_eq!(tools.len(), 20);
        }
    }

    #[test]
    fn sealed_proof_fixture_uses_only_revision_native_fields_and_audited_annotations() {
        for revision in McpRevisionV3::all() {
            let tools = tools_for_surface_v3(*revision, super::super::V3SurfaceSet::WithProof);
            assert_eq!(
                tools.len(),
                21,
                "{revision:?} should include the dark proof tool"
            );
            let expected = revision
                .profile()
                .tool_fields
                .iter()
                .copied()
                .collect::<BTreeSet<_>>();
            for projected in &tools {
                assert_eq!(
                    field_names(projected),
                    expected,
                    "{revision:?}: {projected}"
                );
                assert!(projected.get("safety").is_none());
            }

            if matches!(
                revision,
                McpRevisionV3::March2025 | McpRevisionV3::June2025 | McpRevisionV3::November2025
            ) {
                assert_eq!(
                    tool(&tools, "status").pointer("/annotations/readOnlyHint"),
                    Some(&json!(true))
                );
                assert_eq!(
                    tool(&tools, "prove_call_path").pointer("/annotations/readOnlyHint"),
                    Some(&json!(true))
                );
                for activation_capable in ["packet", "search", "ground", "context"] {
                    assert!(
                        tool(&tools, activation_capable)
                            .pointer("/annotations/readOnlyHint")
                            .is_none(),
                        "activation-capable {activation_capable} must omit readOnlyHint"
                    );
                }
            }

            if matches!(
                revision,
                McpRevisionV3::June2025 | McpRevisionV3::November2025
            ) {
                let status_safety = tool(&tools, "status")
                    .pointer("/_meta/com.thegreencedar.codestory~1safety")
                    .expect("vendor safety metadata");
                assert_eq!(status_safety["effect"], "read_only");
                let packet_safety = tool(&tools, "packet")
                    .pointer("/_meta/com.thegreencedar.codestory~1safety")
                    .expect("vendor safety metadata");
                assert_eq!(packet_safety["effect"], "managed_activation");
            }
        }
    }

    #[test]
    fn modern_output_schemas_are_root_objects_and_accept_maximal_projection_variants() {
        let identity = json!({
            "packet_id":"b96ac0cc-e552-4c35-a0ba-c83b9ead67de",
            "request_id":"request-1",
            "question_sha256":"a".repeat(64)
        });
        let publication = json!({
            "core":{"project_id":"project-1","generation_id":"core-1","run_id":"run-1"},
            "retrieval":{
                "core_generation_id":"core-1",
                "core_run_id":"run-1",
                "retrieval_generation":"retrieval-1",
                "retrieval_input_sha256":"b".repeat(64),
                "semantic_generation":"semantic-1"
            }
        });
        let diagnostics = json!({
            "availability":"available",
            "reference":{
                "artifact_id":"diagnostic-1",
                "sha256":"c".repeat(64),
                "byte_length":128,
                "uri":"codestory://packet-diagnostics/b96ac0cc-e552-4c35-a0ba-c83b9ead67de/".to_string() + &"d".repeat(64),
                "wall_expiry_epoch_ms":1_725_000_600_123_u64
            }
        });
        let maximal = [
            (
                "packet",
                json!({
                    "kind":"complete","schema_version":3,"identity":identity,"publication":publication,
                    "status":"continuation_available","retrieval":{"state":"full","generation_id":"retrieval-1"},
                    "evidence":[{"identity":{"evidence_id":"evidence-1"},"kind":"exact_source","path":"src/lib.rs","symbol_id":"crate::entry","start_line":4,"end_line":9,"summary":"entry calls runtime"}],
                    "gaps":[{"identity":{"gap_id":"gap-1"},"kind":"continuation_required","message":"one round remains"}],
                    "continuation":{"continuation_id":"continuation-1","remaining_rounds":1,"gap_ids":[{"gap_id":"gap-1"}]},
                    "diagnostics":diagnostics
                }),
            ),
            (
                "packet",
                json!({
                    "kind":"budget_exceeded","schema_version":3,"identity":identity,"publication":publication,
                    "status":"unavailable","retrieval":{"state":"degraded","generation_id":"retrieval-1"},
                    "diagnostics":diagnostics,
                    "gaps":[{"identity":{"gap_id":"packet-output-budget-exceeded"},"kind":"output_budget_exceeded","message":null}],
                    "maximum_bytes":16384,"required_complete_bytes":16385
                }),
            ),
            (
                "context",
                json!({
                    "kind":"complete","schema_version":3,"identity":identity,"publication":publication,
                    "status":"available","target":{"path":"src/lib.rs","symbol_id":"crate::entry"},
                    "evidence":[{"identity":{"evidence_id":"evidence-1"},"path":"src/lib.rs","symbol_id":"crate::entry","start_line":4,"end_line":9,"excerpt":"pub fn entry() { runtime(); }"}],
                    "gaps":[],"continuation":null,"diagnostics":diagnostics
                }),
            ),
            (
                "search",
                json!({
                    "kind":"complete","schema_version":3,"identity":identity,"publication":publication,
                    "status":"no_useful_evidence","evidence":[],"gaps":[{"identity":{"gap_id":"gap-1"},"kind":"retrieval_unavailable","message":"retrieval unavailable"}],
                    "continuation":null,"retrieval":{"state":"unavailable","generation_id":null},"diagnostics":diagnostics
                }),
            ),
        ];
        for revision in [McpRevisionV3::June2025, McpRevisionV3::November2025] {
            let tools = tools_for_revision_v3(revision);
            for projected in &tools {
                assert_eq!(
                    projected.pointer("/outputSchema/type"),
                    Some(&json!("object")),
                    "modern output schema must be a root object: {projected}"
                );
            }
            let preparing = json!({
                "kind":"preparing",
                "state":"preparing",
                "retry_after_ms":250,
                "operation":{"stage":"publication"}
            });
            for name in ["packet", "context", "search"] {
                assert!(
                    crate::stdio_arguments::validate_structured_content(
                        &tool(&tools, name)["outputSchema"],
                        &preparing,
                    )
                    .is_ok(),
                    "{name} must admit the successful preparing variant"
                );
            }
            for (name, projection) in &maximal {
                assert!(
                    crate::stdio_arguments::validate_structured_content(
                        &tool(&tools, name)["outputSchema"],
                        projection,
                    )
                    .is_ok(),
                    "{name} must admit its maximal v3 projection: {projection}"
                );
            }
        }

        let complete = json!({
            "kind": "complete",
            "schema_version": 1,
            "domain": "indexed_source_call_path_v1",
            "contract_interpretation": "host_supplied",
            "guard_version": "clause_guard_v1",
            "source_text_sha256": "a".repeat(64),
            "contract_digest": "b".repeat(64),
            "core_publication": {"project_id":"p","generation_id":"g","run_id":"r"},
            "identities": {"files":[],"symbols":[],"evidence":[]},
            "spec": {"start":{"kind":"canonical_id","canonical_id":"A"},"steps":[],"prohibit_traversal_through":[],"exclude_from_projection":[]},
            "clauses": [],
            "disposition": {"kind":"unknown","contract_digest":"b".repeat(64),"gaps":[],"connected_receipts":[]},
            "steps": [],
            "receipts": []
        });
        let budget = json!({
            "kind": "budget_exceeded",
            "schema_version": 1,
            "domain": "indexed_source_call_path_v1",
            "contract_interpretation": "host_supplied",
            "guard_version": "clause_guard_v1",
            "source_text_sha256": "a".repeat(64),
            "contract_digest": "b".repeat(64),
            "core_publication": {"project_id":"p","generation_id":"g","run_id":"r"},
            "disposition": {"kind":"unknown","contract_digest":"b".repeat(64),"gaps":[{"kind":"output_budget_exceeded"}]},
            "cap_bytes": 65536,
            "required_complete_size": 65537
        });
        let schema = proof_output_schema_v3();
        assert!(crate::stdio_arguments::validate_structured_content(&schema, &complete).is_ok());
        assert!(crate::stdio_arguments::validate_structured_content(&schema, &budget).is_ok());
        let mut invalid = complete;
        invalid["kind"] = json!("supported");
        assert!(crate::stdio_arguments::validate_structured_content(&schema, &invalid).is_err());
    }

    #[test]
    fn packet_context_and_search_schemas_reject_open_or_malformed_dto_shapes() {
        let identity = json!({
            "packet_id":"b96ac0cc-e552-4c35-a0ba-c83b9ead67de",
            "request_id":"request-1",
            "question_sha256":"a".repeat(64)
        });
        let publication = json!({
            "core":{"project_id":"project-1","generation_id":"core-1","run_id":"run-1"},
            "retrieval":null
        });
        let diagnostics = json!({"availability":"unavailable"});
        let packet_complete = json!({
            "kind":"complete","schema_version":3,"identity":identity,"publication":publication,
            "status":"available","retrieval":{"state":"full","generation_id":null},
            "evidence":[{
                "identity":{"evidence_id":"evidence-1"},"kind":"exact_source",
                "path":"src/lib.rs","symbol_id":null,"start_line":4,"end_line":9,"summary":null
            }],
            "gaps":[],"continuation":null,"diagnostics":diagnostics
        });
        let packet_budget = json!({
            "kind":"budget_exceeded","schema_version":3,"identity":identity,"publication":publication,
            "status":"unavailable","retrieval":{"state":"degraded","generation_id":null},
            "diagnostics":diagnostics,
            "gaps":[{"identity":{"gap_id":"packet-output-budget-exceeded"},"kind":"output_budget_exceeded","message":null}],
            "maximum_bytes":16384,"required_complete_bytes":16385
        });
        let context = json!({
            "kind":"complete","schema_version":3,"identity":identity,"publication":publication,
            "status":"available","target":{"path":"src/lib.rs","symbol_id":null},
            "evidence":[{
                "identity":{"evidence_id":"evidence-1"},"path":"src/lib.rs","symbol_id":null,
                "start_line":4,"end_line":9,"excerpt":null
            }],
            "gaps":[],"continuation":null,"diagnostics":diagnostics
        });
        let search = json!({
            "kind":"complete","schema_version":3,"identity":identity,"publication":publication,
            "status":"no_useful_evidence","evidence":[{
                "identity":{"evidence_id":"evidence-1"},"path":"src/lib.rs","symbol_id":null,
                "start_line":null,"end_line":null,"excerpt":null
            }],
            "gaps":[],"continuation":null,"retrieval":{"state":"unavailable","generation_id":null},
            "diagnostics":diagnostics
        });
        let tools = tools_for_revision_v3(McpRevisionV3::June2025);
        let cases = [
            (
                "packet",
                packet_complete,
                "/identity/request_id",
                "/evidence/0/start_line",
            ),
            (
                "packet",
                packet_budget.clone(),
                "/identity/request_id",
                "/maximum_bytes",
            ),
            ("context", context, "/target/path", "/evidence/0/start_line"),
            (
                "search",
                search,
                "/retrieval/generation_id",
                "/evidence/0/path",
            ),
        ];
        for (name, valid, required_pointer, typed_pointer) in cases {
            let schema = &tool(&tools, name)["outputSchema"];
            assert!(
                crate::stdio_arguments::validate_structured_content(schema, &valid).is_ok(),
                "closed {name} schema rejected its explicit DTO variant: {valid}"
            );

            let mut missing = valid.clone();
            let (parent, field) = required_pointer.rsplit_once('/').unwrap();
            missing
                .pointer_mut(parent)
                .and_then(Value::as_object_mut)
                .unwrap()
                .remove(field);
            assert!(
                crate::stdio_arguments::validate_structured_content(schema, &missing).is_err(),
                "{name} accepted missing DTO field {required_pointer}: {missing}"
            );

            let mut unknown = valid.clone();
            unknown
                .as_object_mut()
                .unwrap()
                .insert("undeclared".into(), json!(true));
            assert!(
                crate::stdio_arguments::validate_structured_content(schema, &unknown).is_err(),
                "{name} accepted an unknown root field: {unknown}"
            );

            let mut nested_unknown = valid.clone();
            nested_unknown["identity"]
                .as_object_mut()
                .unwrap()
                .insert("undeclared".into(), json!(true));
            assert!(
                crate::stdio_arguments::validate_structured_content(schema, &nested_unknown)
                    .is_err(),
                "{name} accepted an unknown nested field: {nested_unknown}"
            );

            let mut mistyped = valid;
            *mistyped.pointer_mut(typed_pointer).unwrap() = json!(false);
            assert!(
                crate::stdio_arguments::validate_structured_content(schema, &mistyped).is_err(),
                "{name} accepted mistyped DTO field {typed_pointer}: {mistyped}"
            );
        }

        let packet_schema = &tool(&tools, "packet")["outputSchema"];
        for (label, invalid) in [
            ("missing typed gap", {
                let mut invalid = packet_budget.clone();
                invalid["gaps"] = json!([]);
                invalid
            }),
            ("wrong typed gap", {
                let mut invalid = packet_budget.clone();
                invalid["gaps"][0]["kind"] = json!("retrieval_unavailable");
                invalid
            }),
            ("non-unavailable status", {
                let mut invalid = packet_budget;
                invalid["status"] = json!("available");
                invalid
            }),
        ] {
            assert!(
                crate::stdio_arguments::validate_structured_content(packet_schema, &invalid)
                    .is_err(),
                "packet budget fallback accepted {label}: {invalid}"
            );
        }
    }
}
