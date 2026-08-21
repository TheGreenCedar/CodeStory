use serde_json::{Value, json};

use super::{
    StdioV3InternalError,
    profile::{BatchPolicyV3, McpRevisionV3},
};

pub(crate) const PROOF_TOOL_RESULT_MAX_BYTES_V3: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrameResponseV3 {
    None,
    Single(Value),
    Batch(Vec<Value>),
}

pub(crate) fn process_jsonrpc_frame_v3<F>(
    revision: McpRevisionV3,
    frame: &Value,
    mut handler: F,
) -> FrameResponseV3
where
    F: FnMut(&Value) -> Value,
{
    let Some(batch) = frame.as_array() else {
        return dispatch_one_v3(frame, &mut handler)
            .map_or(FrameResponseV3::None, FrameResponseV3::Single);
    };
    if batch.is_empty() || revision.profile().batch_policy == BatchPolicyV3::Reject {
        return FrameResponseV3::Single(jsonrpc_error_v3(
            Value::Null,
            -32600,
            "Invalid request: batches are not supported by the negotiated revision",
        ));
    }

    let responses = batch
        .iter()
        .filter_map(|request| dispatch_one_v3(request, &mut handler))
        .collect::<Vec<_>>();
    if responses.is_empty() {
        FrameResponseV3::None
    } else {
        FrameResponseV3::Batch(responses)
    }
}

fn dispatch_one_v3<F>(request: &Value, handler: &mut F) -> Option<Value>
where
    F: FnMut(&Value) -> Value,
{
    let valid = request.as_object().is_some_and(|request| {
        request.get("jsonrpc") == Some(&json!("2.0"))
            && request.get("method").and_then(Value::as_str).is_some()
    });
    if !valid {
        return Some(jsonrpc_error_v3(
            Value::Null,
            -32600,
            "Invalid request: expected a JSON-RPC 2.0 request object",
        ));
    }
    let response = handler(request);
    request.get("id").map(|_| response)
}

fn jsonrpc_error_v3(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message}
    })
}

pub(crate) fn build_proof_tool_result_v3(
    revision: McpRevisionV3,
    root: &Value,
) -> Result<Value, StdioV3InternalError> {
    let result = build_tool_result_v3(revision, "prove_call_path", root)?;
    let bytes = crate::stdio_transport::v3_serialize_call_tool_result(&result)
        .map_err(|error| StdioV3InternalError::Serialization(error.to_string()))?;
    if bytes.len() <= PROOF_TOOL_RESULT_MAX_BYTES_V3 {
        return Ok(result);
    }

    let fallback = proof_budget_fallback_v3(root, bytes.len())?;
    let fallback_result = build_tool_result_v3(revision, "prove_call_path", &fallback)?;
    let fallback_bytes = crate::stdio_transport::v3_serialize_call_tool_result(&fallback_result)
        .map_err(|error| StdioV3InternalError::Serialization(error.to_string()))?;
    if fallback_bytes.len() > PROOF_TOOL_RESULT_MAX_BYTES_V3 {
        return Err(StdioV3InternalError::ResultExceedsBudget {
            maximum_bytes: PROOF_TOOL_RESULT_MAX_BYTES_V3,
            actual_bytes: fallback_bytes.len(),
        });
    }
    Ok(fallback_result)
}

pub(crate) fn build_tool_result_v3(
    revision: McpRevisionV3,
    tool_name: &str,
    root: &Value,
) -> Result<Value, StdioV3InternalError> {
    let modern_tools = super::catalog::tools_for_revision_v3(McpRevisionV3::June2025);
    let schema = modern_tools
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some(tool_name))
        .and_then(|tool| tool.get("outputSchema"))
        .ok_or_else(|| StdioV3InternalError::InvalidProjection(tool_name.to_string()))?;
    revision_native_tool_result_with_schema_v3(revision, root, schema)
}

fn revision_native_tool_result_with_schema_v3(
    revision: McpRevisionV3,
    root: &Value,
    schema: &Value,
) -> Result<Value, StdioV3InternalError> {
    if revision.profile().structured_content
        && crate::stdio_arguments::validate_structured_content(schema, root).is_err()
    {
        return Err(StdioV3InternalError::OutputSchemaViolation);
    }
    let text = serde_json::to_string(root)
        .map_err(|error| StdioV3InternalError::Serialization(error.to_string()))?;
    if revision.profile().structured_content {
        Ok(json!({
            "content": [{"type":"text","text":text}],
            "structuredContent": root,
            "isError": false,
            "_meta": {
                "com.thegreencedar.codestory/protocolRevision": revision.as_str()
            }
        }))
    } else {
        Ok(json!({
            "content": [{"type":"text","text":text}],
            "isError": false
        }))
    }
}

fn proof_budget_fallback_v3(
    complete: &Value,
    required_complete_size: usize,
) -> Result<Value, StdioV3InternalError> {
    if complete.get("kind") != Some(&json!("complete")) {
        return Err(StdioV3InternalError::ResultExceedsBudget {
            maximum_bytes: PROOF_TOOL_RESULT_MAX_BYTES_V3,
            actual_bytes: required_complete_size,
        });
    }
    let required = |name: &str| {
        complete
            .get(name)
            .cloned()
            .ok_or_else(|| StdioV3InternalError::InvalidProjection(name.to_string()))
    };
    let contract_digest = required("contract_digest")?;
    Ok(json!({
        "kind": "budget_exceeded",
        "schema_version": required("schema_version")?,
        "domain": required("domain")?,
        "contract_interpretation": required("contract_interpretation")?,
        "guard_version": required("guard_version")?,
        "source_text_sha256": required("source_text_sha256")?,
        "contract_digest": contract_digest,
        "core_publication": required("core_publication")?,
        "disposition": {
            "kind": "unknown",
            "contract_digest": contract_digest,
            "gaps": [{"kind":"output_budget_exceeded"}]
        },
        "cap_bytes": PROOF_TOOL_RESULT_MAX_BYTES_V3,
        "required_complete_size": required_complete_size
    }))
}

pub(crate) fn semantic_tool_error_v3(message: &str) -> Value {
    json!({
        "content": [{"type":"text","text":message}],
        "isError": true
    })
}

pub(crate) fn jsonrpc_invalid_params_v3(id: Value, message: &str) -> Value {
    jsonrpc_error_v3(id, -32602, message)
}

pub(crate) fn jsonrpc_internal_error_v3(id: Value, _error: &StdioV3InternalError) -> Value {
    jsonrpc_error_v3(id, -32603, "Internal error")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proof_root(disposition_kind: &str) -> Value {
        let disposition = match disposition_kind {
            "unavailable" => json!({
                "kind": "unavailable",
                "contract_digest": "b".repeat(64),
                "reasons": ["publication_unavailable"]
            }),
            _ => json!({
                "kind": "unknown",
                "contract_digest": "b".repeat(64),
                "gaps": [{"kind":"direct_call_missing"}],
                "connected_receipts": []
            }),
        };
        json!({
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
            "disposition": disposition,
            "steps": [],
            "receipts": []
        })
    }

    #[test]
    fn legacy_batches_preserve_order_and_omit_notifications_while_modern_rejects() {
        let frame = json!([
            {"jsonrpc":"2.0","id":1,"method":"first"},
            {"jsonrpc":"2.0","method":"notify"},
            {"jsonrpc":"2.0","id":2,"method":"second"}
        ]);
        for revision in [McpRevisionV3::November2024, McpRevisionV3::March2025] {
            let mut called = Vec::new();
            let response = process_jsonrpc_frame_v3(revision, &frame, |request| {
                called.push(request["method"].as_str().unwrap().to_string());
                json!({"jsonrpc":"2.0","id":request.get("id").cloned().unwrap_or(Value::Null),"result":{"method":request["method"]}})
            });
            assert_eq!(called, ["first", "notify", "second"]);
            let FrameResponseV3::Batch(responses) = response else {
                panic!("legacy profile must return an ordered response batch")
            };
            assert_eq!(responses.len(), 2);
            assert_eq!(responses[0]["id"], 1);
            assert_eq!(responses[1]["id"], 2);
        }

        for revision in [McpRevisionV3::June2025, McpRevisionV3::November2025] {
            let response = process_jsonrpc_frame_v3(revision, &frame, |_| {
                panic!("modern batch rejection must happen before dispatch")
            });
            let FrameResponseV3::Single(response) = response else {
                panic!("modern batch should be one invalid-request response")
            };
            assert_eq!(response.pointer("/error/code"), Some(&json!(-32600)));
            assert_eq!(response["id"], Value::Null);
        }
    }

    #[test]
    fn result_profiles_are_revision_native_and_keep_typed_uncertainty_successful() {
        for disposition in ["unknown", "unavailable"] {
            let root = proof_root(disposition);
            for revision in McpRevisionV3::all() {
                let result = build_proof_tool_result_v3(*revision, &root).expect("tool result");
                assert_eq!(result["isError"], false);
                let text = result
                    .pointer("/content/0/text")
                    .and_then(Value::as_str)
                    .unwrap();
                assert_eq!(serde_json::from_str::<Value>(text).unwrap(), root);
                if revision.profile().structured_content {
                    assert_eq!(result["structuredContent"], root);
                    assert_eq!(
                        result.pointer("/_meta/com.thegreencedar.codestory~1protocolRevision"),
                        Some(&json!(revision.as_str()))
                    );
                } else {
                    assert!(result.get("structuredContent").is_none());
                    assert!(result.get("_meta").is_none());
                }
            }
        }
    }

    #[test]
    fn post_budget_validation_suppresses_invalid_payload_and_whole_result_falls_back() {
        let mut invalid = proof_root("unknown");
        invalid["undeclared"] = json!(true);
        let error = build_proof_tool_result_v3(McpRevisionV3::June2025, &invalid)
            .expect_err("undeclared output must fail closed");
        assert_eq!(error, StdioV3InternalError::OutputSchemaViolation);
        let response = jsonrpc_internal_error_v3(json!(7), &error);
        assert_eq!(response.pointer("/error/code"), Some(&json!(-32603)));
        assert!(response.pointer("/error/data/structuredContent").is_none());

        let mut oversized = proof_root("unknown");
        oversized["receipts"] = json!([{"line_window":{"text":"\\\"é".repeat(24_000)}}]);
        let result = build_proof_tool_result_v3(McpRevisionV3::June2025, &oversized)
            .expect("fallback result");
        assert_eq!(
            result.pointer("/structuredContent/kind"),
            Some(&json!("budget_exceeded"))
        );
        assert!(result.pointer("/structuredContent/receipts").is_none());
        assert!(
            result
                .pointer("/structuredContent/required_complete_size")
                .and_then(Value::as_u64)
                .is_some_and(|size| size > PROOF_TOOL_RESULT_MAX_BYTES_V3 as u64)
        );
        assert!(
            crate::stdio_transport::v3_serialize_call_tool_result(&result)
                .unwrap()
                .len()
                <= PROOF_TOOL_RESULT_MAX_BYTES_V3
        );
    }

    #[test]
    fn request_shape_semantic_and_internal_errors_keep_distinct_wire_classes() {
        let invalid = jsonrpc_invalid_params_v3(json!(1), "bad arguments");
        assert_eq!(invalid.pointer("/error/code"), Some(&json!(-32602)));

        let semantic = semantic_tool_error_v3("invalid proof interpretation");
        assert_eq!(semantic["isError"], true);
        assert_eq!(
            semantic.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["content", "isError"]
        );
        assert_eq!(semantic.pointer("/content/0/type"), Some(&json!("text")));

        let internal = jsonrpc_internal_error_v3(
            json!(2),
            &StdioV3InternalError::Serialization("secret detail".into()),
        );
        assert_eq!(internal.pointer("/error/code"), Some(&json!(-32603)));
        assert!(!internal.to_string().contains("secret detail"));
    }

    #[test]
    fn preparing_is_a_successful_revision_native_tagged_variant() {
        let preparing = json!({
            "kind": "preparing",
            "state": "preparing",
            "retry_after_ms": 250,
            "operation": {"stage":"publication"}
        });
        let schema = json!({
            "type":"object",
            "properties": {
                "kind":{"type":"string","enum":["preparing"]},
                "state":{"type":"string","enum":["preparing"]},
                "retry_after_ms":{"type":"integer","minimum":1},
                "operation":{"type":"object"}
            },
            "required":["kind","state","retry_after_ms","operation"],
            "additionalProperties":false
        });
        for revision in McpRevisionV3::all() {
            let result = revision_native_tool_result_with_schema_v3(*revision, &preparing, &schema)
                .expect("preparing is successful");
            assert_eq!(result["isError"], false);
            assert_eq!(
                serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap())
                    .unwrap(),
                preparing
            );
            if revision.profile().structured_content {
                assert_eq!(result["structuredContent"], preparing);
            }
        }
    }

    #[test]
    fn diagnostics_token_is_only_mirrored_inside_the_result_uri() {
        let now = std::time::Instant::now();
        let mut registry =
            super::super::diagnostics::DiagnosticsRegistryV3::new_with_secret([8; 32]);
        let artifact = br#"{"kind":"complete","rows":[]}"#.to_vec();
        let grant = registry
            .register_at(
                super::super::diagnostics::DiagnosticsBindingV3 {
                    packet_id: uuid::Uuid::new_v4().to_string(),
                    project_id: "project-1".into(),
                    publication_id: "core-1/retrieval-1".into(),
                    request_digest: "a".repeat(64),
                    wall_expiry_epoch_ms: 99_000,
                },
                artifact,
                now,
            )
            .expect("registered diagnostic artifact");
        let mut projection = json!({
            "kind": "complete",
            "schema_version": 3,
            "diagnostics": {
                "availability": "available",
                "reference": {
                    "artifact_id": "diagnostic-1",
                    "sha256": grant.sha256,
                    "byte_length": grant.byte_length
                }
            }
        });
        super::super::diagnostics::attach_capability_uri_v3(&mut projection, &grant)
            .expect("bind capability URI to finalized projection");
        let token = grant.uri.rsplit('/').next().unwrap();

        for revision in McpRevisionV3::all() {
            let result = build_tool_result_v3(*revision, "packet", &projection)
                .expect("revision-native packet result");
            let mirrored = result
                .pointer("/content/0/text")
                .and_then(Value::as_str)
                .unwrap();
            assert_eq!(serde_json::from_str::<Value>(mirrored).unwrap(), projection);
            assert!(
                !result
                    .get("_meta")
                    .is_some_and(|meta| meta.to_string().contains(token))
            );
            assert_eq!(
                result.to_string().matches(token).count(),
                if revision.profile().structured_content {
                    2
                } else {
                    1
                }
            );
            assert!(
                !super::super::catalog::tools_for_revision_v3(*revision)
                    .iter()
                    .any(|tool| tool.to_string().contains(token))
            );
        }
    }

    #[test]
    fn measurement_seam_owns_exact_builder_bytes_for_every_revision() {
        let root = proof_root("unknown");
        let measured = super::super::measure_revision_native_proof_result_v3(&root)
            .expect("revision-native measurements");
        assert_eq!(measured.len(), 4);
        for measurement in measured {
            let direct = build_proof_tool_result_v3(measurement.revision, &root).unwrap();
            let direct_bytes = crate::stdio_transport::v3_serialize_call_tool_result(&direct)
                .expect("exact tool result bytes");
            assert_eq!(measurement.call_tool_result_bytes, direct_bytes);
            assert_eq!(measurement.byte_length, direct_bytes.len());
        }
    }
}
