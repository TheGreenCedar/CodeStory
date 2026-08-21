use serde_json::{Map, Value, json};

use super::profile::McpRevisionV3;

const VENDOR_SAFETY_KEY: &str = "com.thegreencedar.codestory/safety";

pub(crate) fn tools_for_revision_v3(revision: McpRevisionV3) -> Vec<Value> {
    let mut sources = crate::stdio_catalog::v3_tool_source_json();
    sources.push(proof_tool_source_v3());
    sources
        .into_iter()
        .map(|source| project_tool_v3(revision, &source))
        .collect()
}

pub(crate) fn proof_output_schema_v3() -> Value {
    let common = Map::from_iter([
        ("kind".to_string(), json!({"type":"string"})),
        ("schema_version".to_string(), json!({"type":"integer"})),
        ("domain".to_string(), json!({"type":"string"})),
        (
            "contract_interpretation".to_string(),
            json!({"type":"string","enum":["host_supplied"]}),
        ),
        ("guard_version".to_string(), json!({"type":"string"})),
        (
            "source_text_sha256".to_string(),
            json!({"type":"string","minLength":64,"maxLength":64}),
        ),
        (
            "contract_digest".to_string(),
            json!({"type":"string","minLength":64,"maxLength":64}),
        ),
        ("core_publication".to_string(), json!({"type":"object"})),
        ("disposition".to_string(), json!({"type":"object"})),
    ]);
    let variant = |kind: &str, additional: &[(&str, Value)], required: &[&str]| {
        let mut properties = common.clone();
        properties.insert("kind".to_string(), json!({"type":"string","enum":[kind]}));
        properties.extend(
            additional
                .iter()
                .map(|(name, schema)| ((*name).to_string(), schema.clone())),
        );
        let mut required_fields = vec![
            "kind",
            "schema_version",
            "domain",
            "contract_interpretation",
            "guard_version",
            "source_text_sha256",
            "contract_digest",
            "core_publication",
            "disposition",
        ];
        required_fields.extend_from_slice(required);
        json!({
            "type": "object",
            "properties": properties,
            "required": required_fields,
            "additionalProperties": false
        })
    };
    json!({
        "type": "object",
        "oneOf": [
            variant(
                "complete",
                &[
                    ("spec", json!({"type":"object"})),
                    ("clauses", json!({"type":"array"})),
                    ("steps", json!({"type":"array"})),
                    ("receipts", json!({"type":"array"})),
                ],
                &["spec", "clauses", "steps", "receipts"],
            ),
            variant(
                "budget_exceeded",
                &[
                    ("cap_bytes", json!({"type":"integer","minimum":1})),
                    (
                        "required_complete_size",
                        json!({"type":"integer","minimum":1}),
                    ),
                ],
                &["cap_bytes", "required_complete_size"],
            )
        ]
    })
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
            let mut annotations = Map::from_iter([
                ("destructiveHint".to_string(), json!(false)),
                ("idempotentHint".to_string(), json!(true)),
                ("openWorldHint".to_string(), json!(activates)),
            ]);
            if !activates {
                annotations.insert("readOnlyHint".to_string(), json!(true));
            }
            projected.insert("annotations".to_string(), Value::Object(annotations));
        }
        McpRevisionV3::June2025 | McpRevisionV3::November2025 => {
            projected.insert("title".to_string(), json!(title_v3(name)));
            projected.insert(
                "outputSchema".to_string(),
                output_schema_for_tool_v3(name, source),
            );
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

fn output_schema_for_tool_v3(name: &str, source: &Value) -> Value {
    match name {
        "prove_call_path" => proof_output_schema_v3(),
        "packet" => tagged_evidence_schema_v3(&["complete", "budget_exceeded"]),
        "context" | "search" => tagged_evidence_schema_v3(&["complete"]),
        _ => source
            .get("outputSchema")
            .cloned()
            .unwrap_or_else(|| json!({"type":"object"})),
    }
}

fn tagged_evidence_schema_v3(kinds: &[&str]) -> Value {
    json!({
        "type": "object",
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "kind": {"type":"string","enum":kinds},
                    "schema_version": {"type":"integer"}
                },
                "required": ["kind", "schema_version"]
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
    fn discovery_tools_use_only_revision_native_fields_and_audited_annotations() {
        for revision in McpRevisionV3::all() {
            let tools = tools_for_revision_v3(*revision);
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

            if *revision == McpRevisionV3::March2025 {
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
                "uri":"codestory://packet-diagnostics/b96ac0cc-e552-4c35-a0ba-c83b9ead67de/".to_string() + &"d".repeat(64)
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
                    "diagnostics":diagnostics,"maximum_bytes":16384,"required_complete_bytes":16385
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
}
