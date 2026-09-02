//! Shared public result contract for `verify_indexed_direct_calls`.
//!
//! Runtime owns projection into this DTO. Transport adapters may serialize it
//! and advertise this schema, but must not reinterpret or rebuild it.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const PUBLIC_CALL_PATH_DOMAIN: &str = "call-path/v1";

/// Runtime-produced public verification result.
///
/// The proof kernel already validates the complete internal projection. This
/// wrapper prevents adapters from accepting an arbitrary JSON value as the
/// shared result and fixes the public invariants that differ from the internal
/// representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PublicCallPathResultDto(Value);

impl PublicCallPathResultDto {
    pub fn try_from_projected_value(value: Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "public call-path result root must be an object".to_owned())?;
        let kind = required_string(object, "kind")?;
        if !matches!(kind, "complete" | "budget_exceeded") {
            return Err(format!("unsupported public call-path result kind `{kind}`"));
        }
        if object.get("schema_version").and_then(Value::as_u64) != Some(1) {
            return Err("public call-path result schema_version must be 1".to_owned());
        }
        if required_string(object, "domain")? != PUBLIC_CALL_PATH_DOMAIN {
            return Err("public call-path result domain must be call-path/v1".to_owned());
        }
        if required_string(object, "translation_status")? != "host_supplied" {
            return Err(
                "public call-path result translation_status must be host_supplied".to_owned(),
            );
        }
        if !matches!(
            required_string(object, "graph_disposition")?,
            "proven" | "refuted" | "unknown"
        ) {
            return Err("public call-path result has invalid graph_disposition".to_owned());
        }
        if object
            .get("runtime_execution_proven")
            .and_then(Value::as_bool)
            != Some(false)
        {
            return Err("public call-path result cannot claim runtime execution".to_owned());
        }
        for hash in ["source_text_sha256", "contract_digest"] {
            if required_string(object, hash)?.len() != 64 {
                return Err(format!(
                    "public call-path result {hash} must be a SHA-256 digest"
                ));
            }
        }
        for required in [
            "guard_version",
            "core_publication",
            "provenance",
            "disposition",
        ] {
            if !object.contains_key(required) {
                return Err(format!("public call-path result is missing `{required}`"));
            }
        }
        if kind == "complete" {
            for required in ["identities", "spec", "clauses", "steps", "receipts"] {
                if !object.contains_key(required) {
                    return Err(format!(
                        "complete public call-path result is missing `{required}`"
                    ));
                }
            }
        } else {
            if object.get("cap_bytes").and_then(Value::as_u64).is_none()
                || object
                    .get("required_complete_size")
                    .and_then(Value::as_u64)
                    .is_none()
            {
                return Err(
                    "budget-exceeded public call-path result is missing byte accounting".to_owned(),
                );
            }
        }
        Ok(Self(value))
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn into_value(self) -> Value {
        self.0
    }
}

fn required_string<'a>(object: &'a Map<String, Value>, field: &str) -> Result<&'a str, String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("public call-path result `{field}` must be a string"))
}

/// JSON schema advertised by every transport for the shared result DTO.
pub fn public_call_path_result_schema() -> Value {
    json!({
        "type": "object",
        "oneOf": [complete_schema(), budget_exceeded_schema()]
    })
}

fn common_fields(kind: &str) -> Vec<(&str, Value)> {
    vec![
        ("kind", enum_schema(&[kind])),
        ("schema_version", json!({"type":"integer","enum":[1]})),
        ("domain", enum_schema(&[PUBLIC_CALL_PATH_DOMAIN])),
        ("translation_status", enum_schema(&["host_supplied"])),
        (
            "graph_disposition",
            enum_schema(&["proven", "refuted", "unknown"]),
        ),
        (
            "runtime_execution_proven",
            json!({"type":"boolean","enum":[false]}),
        ),
        ("guard_version", enum_schema(&["clause_guard_v1"])),
        ("source_text_sha256", sha256_schema()),
        ("contract_digest", sha256_schema()),
        (
            "core_publication",
            closed_object_schema(vec![
                ("project_id", string_schema()),
                ("generation_id", string_schema()),
                ("run_id", string_schema()),
            ]),
        ),
        ("provenance", provenance_capability_schema()),
    ]
}

fn provenance_capability_schema() -> Value {
    closed_object_schema(vec![("availability", enum_schema(&["unavailable"]))])
}

fn complete_schema() -> Value {
    let mut fields = common_fields("complete");
    fields.extend([
        ("disposition", disposition_schema()),
        (
            "identities",
            closed_object_schema(vec![
                (
                    "files",
                    json!({"type":"array","items":file_schema(),"maxItems":65536}),
                ),
                (
                    "symbols",
                    json!({"type":"array","items":symbol_schema(),"maxItems":65536}),
                ),
                (
                    "provenance_profiles",
                    json!({"type":"array","items":provenance_profile_schema(),"maxItems":6}),
                ),
                (
                    "evidence",
                    json!({"type":"array","items":evidence_schema(),"maxItems":65536}),
                ),
            ]),
        ),
        ("spec", spec_schema()),
        (
            "clauses",
            json!({"type":"array","items":clause_schema(),"minItems":1}),
        ),
        (
            "steps",
            json!({"type":"array","items":step_schema(),"maxItems":6}),
        ),
        (
            "receipts",
            json!({"type":"array","items":receipt_schema(),"maxItems":6}),
        ),
    ]);
    closed_object_schema(fields)
}

fn budget_exceeded_schema() -> Value {
    let mut fields = common_fields("budget_exceeded");
    fields.extend([
        ("disposition", budget_disposition_schema()),
        ("cap_bytes", json!({"type":"integer","minimum":1})),
        (
            "required_complete_size",
            json!({"type":"integer","minimum":1}),
        ),
    ]);
    closed_object_schema(fields)
}

fn disposition_schema() -> Value {
    let receipt_sequence = || {
        json!({
            "type":"array",
            "items":unsigned_integer_schema(),
            "maxItems":6,
            "uniqueItems":true
        })
    };
    json!({
        "type":"object",
        "oneOf":[
            closed_object_schema(vec![
                ("kind", enum_schema(&["contract_proven"])),
                ("contract_digest", sha256_schema()),
                ("receipts", receipt_sequence()),
            ]),
            closed_object_schema(vec![
                ("kind", enum_schema(&["unknown"])),
                ("contract_digest", sha256_schema()),
                ("gaps", json!({"type":"array","items":gap_schema(),"minItems":1,"maxItems":256,"uniqueItems":true})),
                ("connected_receipts", receipt_sequence()),
            ]),
            closed_object_schema(vec![
                ("kind", enum_schema(&["contract_refuted"])),
                ("contract_digest", sha256_schema()),
                ("refutation", refutation_schema()),
            ]),
            closed_object_schema(vec![
                ("kind", enum_schema(&["unavailable"])),
                ("contract_digest", sha256_schema()),
                ("reasons", json!({
                    "type":"array",
                    "items":enum_schema(&[
                        "validated_contract_hash_mismatch",
                        "publication_pin_mismatch",
                        "source_not_bound_to_publication",
                        "proof_facts_unavailable",
                        "proof_semantic_projection_unavailable"
                    ]),
                    "minItems":1,
                    "maxItems":5,
                    "uniqueItems":true
                })),
            ]),
        ]
    })
}

fn refutation_schema() -> Value {
    json!({
        "type":"object",
        "oneOf":[
            closed_object_schema(vec![
                ("kind", enum_schema(&["prohibited_scope_traversal"])),
                ("step_index", json!({"type":"integer","minimum":0,"maximum":5})),
                ("prohibition_index", json!({"type":"integer","minimum":0,"maximum":15})),
                ("connected_receipts", json!({"type":"array","items":unsigned_integer_schema(),"maxItems":6,"uniqueItems":true})),
            ]),
        ]
    })
}

fn gap_schema() -> Value {
    let selector_gap = |kind| {
        closed_object_schema(vec![
            ("kind", enum_schema(&[kind])),
            (
                "selector_index",
                json!({"type":"integer","minimum":0,"maximum":6}),
            ),
        ])
    };
    let step_gap = |kind| {
        closed_object_schema(vec![
            ("kind", enum_schema(&[kind])),
            (
                "step_index",
                json!({"type":"integer","minimum":0,"maximum":5}),
            ),
        ])
    };
    json!({
        "type":"object",
        "oneOf":[
            closed_object_schema(vec![("kind", enum_schema(&["unclassified_source_text"]))]),
            closed_object_schema(vec![
                ("kind", enum_schema(&["unresolved_material_clause"])),
                ("clause_id", string_schema()),
                ("reason", enum_schema(&[
                    "missing_selector_resolution",
                    "ambiguous_selector_resolution",
                    "unsupported_interpretation",
                ])),
            ]),
            closed_object_schema(vec![
                ("kind", enum_schema(&["material_token_misclassified"])),
                ("clause_id", string_schema()),
                ("guard_families", json!({
                    "type":"array",
                    "items":enum_schema(&[
                        "quoted_or_backticked_identifier",
                        "arrow_or_relation_notation",
                        "directness",
                        "ordering_or_ordinal",
                        "only",
                        "negation_or_exclusion",
                        "path_like_string",
                        "qualified_symbol_notation",
                    ]),
                    "minItems":1,
                    "maxItems":8,
                    "uniqueItems":true,
                })),
            ]),
            selector_gap("selector_missing"),
            selector_gap("selector_ambiguous"),
            selector_gap("non_callable_selector"),
            step_gap("direct_call_missing"),
            step_gap("recursive_call_not_representable"),
            step_gap("source_window_too_large"),
            step_gap("invalid_utf8"),
            step_gap("source_line_out_of_range"),
            step_gap("edge_containment_unproven"),
            step_gap("missing_direct_call_receipt"),
            step_gap("receipt_or_edge_already_used"),
            step_gap("projection_exclusion_conflicts_with_required_receipt")
        ]
    })
}

fn budget_disposition_schema() -> Value {
    closed_object_schema(vec![
        ("kind", enum_schema(&["unknown"])),
        ("contract_digest", sha256_schema()),
        (
            "gaps",
            json!({
                "type":"array",
                "items":closed_object_schema(vec![("kind", enum_schema(&["output_budget_exceeded"]))]),
                "minItems":1,
                "maxItems":1
            }),
        ),
    ])
}

fn file_schema() -> Value {
    closed_object_schema(vec![
        ("file_node_id", nullable_schema(string_schema())),
        (
            "project_file_components",
            nullable_schema(json!({"type":"array","items":string_schema()})),
        ),
        ("indexed_sha256", nullable_schema(sha256_schema())),
        ("observed_sha256", nullable_schema(sha256_schema())),
    ])
}

fn symbol_schema() -> Value {
    closed_object_schema(vec![
        ("node_id", string_schema()),
        ("canonical_id", nullable_schema(string_schema())),
        ("qualified_name", nullable_schema(string_schema())),
        ("file", nullable_schema(unsigned_integer_schema())),
    ])
}

fn evidence_schema() -> Value {
    closed_object_schema(vec![
        ("fact_id", sha256_schema()),
        ("caller", unsigned_integer_schema()),
        ("target", unsigned_integer_schema()),
        ("edge_id", string_schema()),
        ("callsite_identity", string_schema()),
        (
            "chain",
            json!({"type":"array","items":closed_object_schema(vec![
                ("kind", string_schema()),
                ("symbols", json!({"type":"array","items":unsigned_integer_schema()})),
            ])}),
        ),
        (
            "provenance",
            closed_object_schema(vec![
                ("profile", unsigned_integer_schema()),
                (
                    "dependency_files",
                    json!({"type":"array","items":unsigned_integer_schema()}),
                ),
                ("evidence_sha256", sha256_schema()),
            ]),
        ),
    ])
}

fn provenance_profile_schema() -> Value {
    closed_object_schema(vec![
        ("producer", enum_schema(&["codestory-internal"])),
        ("fact_schema_version", json!({"type":"integer","enum":[1]})),
        ("algorithm", enum_schema(&["exact-call-resolution-v1"])),
        ("language_adapter", string_schema()),
        ("language_adapter_version", string_schema()),
        ("parser_fingerprint", sha256_schema()),
    ])
}

fn spec_schema() -> Value {
    closed_object_schema(vec![
        ("start", symbol_selector_schema()),
        (
            "steps",
            json!({
                "type":"array",
                "items":closed_object_schema(vec![
                    ("relation", enum_schema(&["direct_outgoing_call"])),
                    ("target", symbol_selector_schema()),
                ]),
                "minItems":1,
                "maxItems":6
            }),
        ),
        (
            "prohibit_traversal_through",
            json!({"type":"array","items":inline_selector_schema(),"maxItems":16}),
        ),
        (
            "exclude_from_projection",
            json!({"type":"array","items":inline_selector_schema(),"maxItems":16}),
        ),
    ])
}

fn symbol_selector_schema() -> Value {
    let mut variants = inline_selector_variants();
    variants.extend([
        closed_object_schema(vec![
            ("kind", enum_schema(&["pinned_node_ref"])),
            ("symbol", unsigned_integer_schema()),
        ]),
        closed_object_schema(vec![
            ("kind", enum_schema(&["canonical_id_ref"])),
            ("symbol", unsigned_integer_schema()),
        ]),
        closed_object_schema(vec![
            ("kind", enum_schema(&["qualified_name_ref"])),
            ("symbol", unsigned_integer_schema()),
            ("path_binding", enum_schema(&["none", "exact_file"])),
        ]),
    ]);
    json!({"type":"object","oneOf":variants})
}

fn inline_selector_schema() -> Value {
    json!({"type":"object","oneOf":inline_selector_variants()})
}

fn inline_selector_variants() -> Vec<Value> {
    vec![
        closed_object_schema(vec![
            ("kind", enum_schema(&["pinned_node"])),
            ("project_id", string_schema()),
            ("core_generation_id", string_schema()),
            ("core_run_id", string_schema()),
            ("node_id", string_schema()),
        ]),
        closed_object_schema(vec![
            ("kind", enum_schema(&["canonical_id"])),
            ("canonical_id", string_schema()),
        ]),
        closed_object_schema(vec![
            ("kind", enum_schema(&["qualified_name"])),
            ("qualified_name", string_schema()),
            (
                "project_file_components",
                nullable_schema(json!({"type":"array","items":string_schema(),"minItems":1})),
            ),
        ]),
    ]
}

fn clause_schema() -> Value {
    closed_object_schema(vec![
        ("start", unsigned_integer_schema()),
        ("end", unsigned_integer_schema()),
        ("clause_id", string_schema()),
        ("quote", string_schema()),
        (
            "classification",
            enum_schema(&["resolved_material", "unresolved_material", "non_material"]),
        ),
        (
            "fields",
            json!({"type":"array","items":contract_field_schema(),"maxItems":57}),
        ),
        (
            "reason",
            nullable_schema(enum_schema(&[
                "missing_selector_resolution",
                "ambiguous_selector_resolution",
                "unsupported_interpretation",
            ])),
        ),
        (
            "non_material_kind",
            nullable_schema(enum_schema(&[
                "whitespace",
                "punctuation",
                "connector",
                "commentary",
            ])),
        ),
    ])
}

fn contract_field_schema() -> Value {
    json!({
        "type":"object",
        "oneOf":[
            closed_object_schema(vec![("kind", enum_schema(&["start"]))]),
            closed_object_schema(vec![
                ("kind", enum_schema(&["step_target","directness","ordering","relation"])),
                ("step", unsigned_integer_schema()),
            ]),
            closed_object_schema(vec![
                ("kind", enum_schema(&["traversal_prohibition","projection_exclusion"])),
                ("index", unsigned_integer_schema()),
            ]),
        ]
    })
}

fn receipt_schema() -> Value {
    closed_object_schema(vec![
        ("receipt_id", string_schema()),
        ("edge_id", string_schema()),
        ("source", unsigned_integer_schema()),
        ("target", unsigned_integer_schema()),
        ("evidence", unsigned_integer_schema()),
        ("exact_callsite_start_byte", unsigned_integer_schema()),
        ("callsite_identity", string_schema()),
        ("column_or_ordinal", unsigned_integer_schema()),
        (
            "containment",
            closed_object_schema(vec![
                ("file", unsigned_integer_schema()),
                ("owner", unsigned_integer_schema()),
                ("start_line", unsigned_integer_schema()),
                ("end_line", unsigned_integer_schema()),
            ]),
        ),
        (
            "line_window",
            closed_object_schema(vec![
                ("kind", enum_schema(&["indexed_line_v1"])),
                ("file", unsigned_integer_schema()),
                ("anchor_line", unsigned_integer_schema()),
                ("byte_start", unsigned_integer_schema()),
                ("byte_end", unsigned_integer_schema()),
                ("text", string_schema()),
            ]),
        ),
    ])
}

fn step_schema() -> Value {
    closed_object_schema(vec![
        ("step_index", unsigned_integer_schema()),
        (
            "status",
            enum_schema(&["proven", "positive_contradiction", "unavailable", "unknown"]),
        ),
        ("receipt", nullable_schema(unsigned_integer_schema())),
    ])
}

fn closed_object_schema(properties: Vec<(&str, Value)>) -> Value {
    let required = properties
        .iter()
        .map(|(name, _)| Value::String((*name).to_owned()))
        .collect::<Vec<_>>();
    let properties = properties
        .into_iter()
        .map(|(name, schema)| (name.to_owned(), schema))
        .collect::<Map<_, _>>();
    json!({
        "type":"object",
        "properties":properties,
        "required":required,
        "additionalProperties":false
    })
}

fn nullable_schema(schema: Value) -> Value {
    json!({"anyOf":[schema,{"type":"null"}]})
}

fn enum_schema(values: &[&str]) -> Value {
    json!({"type":"string","enum":values})
}

fn string_schema() -> Value {
    json!({"type":"string"})
}

fn sha256_schema() -> Value {
    json!({"type":"string","minLength":64,"maxLength":64})
}

fn unsigned_integer_schema() -> Value {
    json!({"type":"integer","minimum":0})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_result_rejects_runtime_and_domain_reinterpretation() {
        let base = json!({
            "kind":"budget_exceeded",
            "schema_version":1,
            "domain":"call-path/v1",
            "translation_status":"host_supplied",
            "graph_disposition":"unknown",
            "runtime_execution_proven":false,
            "guard_version":"clause_guard_v1",
            "source_text_sha256":"a".repeat(64),
            "contract_digest":"b".repeat(64),
            "core_publication":{},
            "provenance":{"availability":"unavailable"},
            "disposition":{},
            "cap_bytes":4096,
            "required_complete_size":5000
        });
        PublicCallPathResultDto::try_from_projected_value(base.clone()).unwrap();
        for (pointer, replacement) in [
            ("domain", json!("other")),
            ("translation_status", json!("inferred")),
            ("runtime_execution_proven", json!(true)),
        ] {
            let mut hostile = base.clone();
            hostile[pointer] = replacement;
            assert!(PublicCallPathResultDto::try_from_projected_value(hostile).is_err());
        }
    }

    #[test]
    fn schema_is_closed_and_owns_the_compact_variants() {
        let schema = public_call_path_result_schema();
        assert_eq!(
            schema.pointer("/oneOf/0/additionalProperties"),
            Some(&json!(false))
        );
        assert_eq!(
            schema.pointer("/oneOf/1/additionalProperties"),
            Some(&json!(false))
        );
        assert_eq!(
            schema.pointer("/oneOf/0/properties/domain/enum/0"),
            Some(&json!("call-path/v1"))
        );
    }
}
