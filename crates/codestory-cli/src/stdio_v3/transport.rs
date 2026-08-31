use serde_json::{Value, json};

use super::{
    StdioV3InternalError, V3SurfaceSet,
    profile::{BatchPolicyV3, McpRevisionV3},
};

pub(crate) const PROOF_TOOL_RESULT_MAX_BYTES_V3: usize = 8 * 1024;

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
        return FrameResponseV3::Single(jsonrpc_error_v3(Value::Null, -32600, "Invalid Request"));
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
    let valid = request.as_object().is_some_and(|members| {
        members
            .keys()
            .all(|field| matches!(field.as_str(), "jsonrpc" | "id" | "method" | "params"))
            && members.get("jsonrpc") == Some(&json!("2.0"))
            && members.get("method").and_then(Value::as_str).is_some()
            && members.get("id").is_none_or(is_valid_jsonrpc_id_v3)
            && members
                .get("params")
                .is_none_or(|params| params.is_object() || params.is_array())
    });
    if !valid {
        return Some(jsonrpc_error_v3(
            request
                .get("id")
                .filter(|id| is_valid_jsonrpc_id_v3(id))
                .cloned()
                .unwrap_or(Value::Null),
            -32600,
            "Invalid Request",
        ));
    }
    let response = handler(request);
    request.get("id").map(|_| response)
}

fn is_valid_jsonrpc_id_v3(value: &Value) -> bool {
    value.is_null() || value.is_string() || value.is_number()
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
    internal_root: &Value,
) -> Result<Value, StdioV3InternalError> {
    let tool_name = crate::prove_call_path::PUBLIC_VERIFY_TOOL_NAME;
    codestory_runtime::proof_qualification_support::validate_compact_projection(internal_root)
        .map_err(|_| StdioV3InternalError::InvalidProjection(tool_name.to_owned()))?;
    let public_root =
        crate::prove_call_path::project_public_verification_result(internal_root.clone())
            .map_err(|_| StdioV3InternalError::InvalidProjection(tool_name.to_owned()))?;
    if crate::stdio_arguments::validate_structured_content(
        &super::catalog::proof_output_schema_v3(),
        &public_root,
    )
    .is_err()
    {
        return Err(StdioV3InternalError::OutputSchemaViolation);
    }
    let result = build_tool_result_for_surface_v3(
        revision,
        tool_name,
        &public_root,
        V3SurfaceSet::WithProof,
    )?;
    let bytes = crate::stdio_transport::v3_serialize_call_tool_result(&result)
        .map_err(|error| StdioV3InternalError::Serialization(error.to_string()))?;
    if bytes.len() <= PROOF_TOOL_RESULT_MAX_BYTES_V3 {
        return Ok(result);
    }

    let fallback_internal = proof_budget_fallback_v3(internal_root, bytes.len())?;
    let fallback_public =
        crate::prove_call_path::project_public_verification_result(fallback_internal)
            .map_err(|_| StdioV3InternalError::InvalidProjection(tool_name.to_owned()))?;
    if crate::stdio_arguments::validate_structured_content(
        &super::catalog::proof_output_schema_v3(),
        &fallback_public,
    )
    .is_err()
    {
        return Err(StdioV3InternalError::OutputSchemaViolation);
    }
    let fallback_result = build_tool_result_for_surface_v3(
        revision,
        tool_name,
        &fallback_public,
        V3SurfaceSet::WithProof,
    )?;
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
    if crate::prove_call_path::is_proof_tool_name(tool_name) {
        return build_tool_result_for_surface_v3(
            revision,
            crate::prove_call_path::PUBLIC_VERIFY_TOOL_NAME,
            root,
            V3SurfaceSet::WithProof,
        );
    }
    build_tool_result_for_surface_v3(revision, tool_name, root, V3SurfaceSet::EvidenceOnly)
}

fn build_tool_result_for_surface_v3(
    revision: McpRevisionV3,
    tool_name: &str,
    root: &Value,
    surface: V3SurfaceSet,
) -> Result<Value, StdioV3InternalError> {
    let modern_tools = super::catalog::tools_for_surface_v3(McpRevisionV3::June2025, surface);
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
        && let Err(violations) = crate::stdio_arguments::validate_structured_content(schema, root)
    {
        let _ = violations;
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
                "com.thegreencedar.codestory/protocolRevision": revision.as_str(),
                "codestory_publication": {
                    "schema_version": codestory_contracts::wire::PUBLICATION_STAMP_SCHEMA_VERSION,
                    "minimum_compatible_schema_version": codestory_contracts::wire::MINIMUM_COMPATIBLE_PUBLICATION_STAMP_SCHEMA_VERSION
                }
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
    use codestory_contracts::graph::NodeId;
    use codestory_contracts::proof_resolution::{
        DependencyFileHash, ResolutionEvidence, ResolutionProvenance,
    };
    use codestory_runtime::proof_qualification_support::{
        BuiltCallPathFacts, ClauseAnchor, ClauseClassification, FactBuildGap,
        InternalCorePublicationIdentity, InternalProjection, ProofContractField, ProofHashes,
        UnavailableReason, UnvalidatedCallPathContract, UnvalidatedCallPathSpec,
        UnvalidatedDirectCallStep, UnvalidatedExactSymbolSelector, ValidatedCallPathContract,
        ValidatedContractRendering, ValidationOutcome, check_built_call_path_integration,
        project_internal_call_path_result, validate_contract,
    };
    use codestory_runtime::proof_qualification_support::{
        CallableContainmentEvidence, IndexedCallEdgeReceipt, IndexedLineWindow, PinnedNodeIdentity,
        ReceiptRef, ResolvedNodeIdentity, VerifiedDirectCallFact, VerifiedProofFact,
    };
    use sha2::{Digest, Sha256};

    fn resolution_fact_id(evidence_sha256: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"codestory-proof-resolution-fact-id-v1\0");
        hasher.update((evidence_sha256.len() as u64).to_be_bytes());
        hasher.update(evidence_sha256.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn validated_test_contract() -> (
        ValidatedCallPathContract,
        ProofHashes,
        ValidatedContractRendering,
    ) {
        let source = "exact direct ordered call path";
        let ValidationOutcome::Validated {
            contract,
            hashes,
            rendering,
        } = validate_contract(UnvalidatedCallPathContract::new(
            source,
            vec![ClauseAnchor {
                clause_id: "contract".to_owned(),
                start: 0,
                end: source.len(),
                quote: source.to_owned(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![
                        ProofContractField::Start,
                        ProofContractField::StepTarget { step: 0 },
                        ProofContractField::Directness { step: 0 },
                        ProofContractField::Ordering { step: 0 },
                        ProofContractField::Relation { step: 0 },
                    ],
                },
            }],
            UnvalidatedCallPathSpec {
                start: UnvalidatedExactSymbolSelector::CanonicalId("A".to_owned()),
                steps: vec![UnvalidatedDirectCallStep {
                    target: UnvalidatedExactSymbolSelector::CanonicalId("B".to_owned()),
                }],
                prohibit_traversal_through: Vec::new(),
                exclude_from_projection: Vec::new(),
            },
        ))
        .expect("valid contract")
        else {
            panic!("fixture contract validates")
        };
        (*contract, hashes, rendering)
    }

    fn actual_projected_root(text: String) -> Value {
        let (contract, hashes, rendering) = validated_test_contract();
        let node = |node_id: &str, canonical_id: &str, qualified_name: &str, path: &[&str]| {
            ResolvedNodeIdentity::new(
                PinnedNodeIdentity {
                    project_id: "project".to_owned(),
                    core_generation_id: "generation".to_owned(),
                    core_run_id: "run".to_owned(),
                    node_id: node_id.to_owned(),
                },
                canonical_id,
                qualified_name,
                NodeId(if node_id == "10" { 1 } else { 2 }),
                path.iter().map(|part| (*part).to_owned()).collect(),
            )
            .expect("valid node")
        };
        let source = node("10", "A", "crate::A", &["src", "a.rs"]);
        let target = node("20", "B", "crate::B", &["src", "b.rs"]);
        let receipt = IndexedCallEdgeReceipt {
            receipt: ReceiptRef {
                receipt_id: "receipt-0".to_owned(),
                edge_id: "1".to_owned(),
            },
            source: source.clone(),
            target: target.clone(),
            resolution_fact_id: resolution_fact_id(&"b".repeat(64)),
            resolution_evidence_sha256: "b".repeat(64),
            resolution_evidence_chain: vec![ResolutionEvidence::SameFileDeclaration {
                declaration: NodeId(20),
            }],
            resolution_provenance: ResolutionProvenance {
                producer: "codestory-internal".to_owned(),
                fact_schema_version: 1,
                algorithm: "exact-call-resolution-v1".to_owned(),
                language_adapter: "rust".to_owned(),
                language_adapter_version: "test-v1".to_owned(),
                parser_fingerprint: "c".repeat(64),
                dependency_file_hashes: vec![
                    DependencyFileHash {
                        file_id: codestory_contracts::proof_resolution::FileId(1),
                        source_sha256: "d".repeat(64),
                    },
                    DependencyFileHash {
                        file_id: codestory_contracts::proof_resolution::FileId(2),
                        source_sha256: "e".repeat(64),
                    },
                ],
                evidence_sha256: "b".repeat(64),
            },
            exact_callsite_start_byte: 0,
            callsite_identity: "1:1:1:20|rust".to_owned(),
            column_or_ordinal: 1,
            containment: CallableContainmentEvidence {
                file_node_id: NodeId(1),
                owner_node_id: NodeId(10),
                start_line: 1,
                end_line: 1,
            },
            line_window: IndexedLineWindow {
                kind: "indexed_line_v1",
                project_file_components: vec!["src".to_owned(), "a.rs".to_owned()],
                indexed_sha256: "d".repeat(64),
                observed_sha256: "d".repeat(64),
                anchor_line: 1,
                byte_start: 0,
                byte_end: text.len(),
                text,
            },
        };
        let integration = check_built_call_path_integration(
            &contract,
            &hashes,
            &rendering,
            BuiltCallPathFacts {
                publication: InternalCorePublicationIdentity {
                    project_id: "project".to_owned(),
                    generation_id: "generation".to_owned(),
                    run_id: "run".to_owned(),
                },
                facts: vec![VerifiedProofFact::DirectCall(VerifiedDirectCallFact {
                    receipt: receipt.receipt.clone(),
                    source,
                    target,
                })],
                receipts: vec![receipt],
                gaps: Vec::new(),
                unavailable: Vec::new(),
            },
        )
        .expect("checked integration");
        let InternalProjection::Complete { root, .. } =
            project_internal_call_path_result(&integration).expect("actual projected root")
        else {
            panic!("fixture must preserve its complete root for transport")
        };
        root
    }

    fn set_receipt_line_text(root: &mut Value, text: String) {
        let byte_start = root["receipts"][0]["line_window"]["byte_start"]
            .as_u64()
            .expect("line byte start");
        root["receipts"][0]["line_window"]["text"] = json!(text);
        root["receipts"][0]["line_window"]["byte_end"] = json!(
            byte_start
                + u64::try_from(
                    root["receipts"][0]["line_window"]["text"]
                        .as_str()
                        .expect("line text")
                        .len()
                )
                .expect("line text length")
        );
    }

    #[cfg(feature = "proof-qualification-support")]
    #[test]
    fn actual_projected_root_round_trips_through_all_revision_profiles() {
        let internal = actual_projected_root("A calls B();\n".to_owned());
        let public = crate::prove_call_path::project_public_verification_result(internal.clone())
            .expect("project public verification result");
        for revision in McpRevisionV3::all() {
            let result = build_proof_tool_result_v3(*revision, &internal)
                .unwrap_or_else(|error| panic!("{revision:?}: {error:?}"));
            assert_eq!(
                serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap())
                    .unwrap(),
                public
            );
            if revision.profile().structured_content {
                assert_eq!(result["structuredContent"], public);
            }
        }
    }

    #[cfg(feature = "proof-qualification-support")]
    #[test]
    fn actual_projector_leaves_an_oversized_complete_root_for_revision_transport() {
        let root = actual_projected_root("\\\"é".repeat(24_000));
        assert_eq!(root["kind"], "complete");
    }

    #[cfg(feature = "proof-qualification-support")]
    fn actual_projected_root_at_revision_bytes(revision: McpRevisionV3, target: usize) -> Value {
        let root = actual_projected_root("A calls B();\n".to_owned());
        let size = |root: &Value| {
            let public = crate::prove_call_path::project_public_verification_result(root.clone())
                .expect("project public verification result for size probe");
            let result = build_tool_result_for_surface_v3(
                revision,
                crate::prove_call_path::PUBLIC_VERIFY_TOOL_NAME,
                &public,
                V3SurfaceSet::WithProof,
            )
            .expect("unbounded revision-native result");
            crate::stdio_transport::v3_serialize_call_tool_result(&result)
                .expect("unbounded revision-native bytes")
                .len()
        };
        let baseline = size(&root);
        assert!(baseline < target);
        for quote_count in 0..=16 {
            let mut candidate = root.clone();
            set_receipt_line_text(&mut candidate, "\"".repeat(quote_count));
            let candidate_size = size(&candidate);
            let mut one_more = candidate.clone();
            set_receipt_line_text(&mut one_more, format!("{}x", "\"".repeat(quote_count)));
            let byte_step = size(&one_more) - candidate_size;
            let remaining = target.saturating_sub(candidate_size);
            if byte_step > 0 && remaining % byte_step == 0 {
                let mut count = remaining / byte_step;
                for _ in 0..4 {
                    set_receipt_line_text(
                        &mut candidate,
                        format!("{}{}", "\"".repeat(quote_count), "x".repeat(count)),
                    );
                    let actual = size(&candidate);
                    if actual == target {
                        return candidate;
                    }
                    let delta = isize::try_from(target).unwrap() - isize::try_from(actual).unwrap();
                    let adjustment = delta / isize::try_from(byte_step).unwrap();
                    let Some(next) = count.checked_add_signed(adjustment) else {
                        break;
                    };
                    if next == count {
                        break;
                    }
                    count = next;
                }
            }
        }
        panic!("revision {revision:?} cannot reach target {target}");
    }

    #[cfg(feature = "proof-qualification-support")]
    #[test]
    fn revision_transport_owns_actual_projected_root_budgeting_and_internal_errors() {
        for revision in McpRevisionV3::all() {
            let fitting_size = if revision.profile().structured_content {
                PROOF_TOOL_RESULT_MAX_BYTES_V3 - 1
            } else {
                PROOF_TOOL_RESULT_MAX_BYTES_V3
            };
            let complete = actual_projected_root_at_revision_bytes(*revision, fitting_size);
            let complete_result = build_proof_tool_result_v3(*revision, &complete).unwrap();
            assert_eq!(
                crate::stdio_transport::v3_serialize_call_tool_result(&complete_result)
                    .unwrap()
                    .len(),
                fitting_size
            );

            let oversized = actual_projected_root_at_revision_bytes(
                *revision,
                PROOF_TOOL_RESULT_MAX_BYTES_V3 + 1,
            );
            let oversized_public =
                crate::prove_call_path::project_public_verification_result(oversized.clone())
                    .expect("project oversized public verification result");
            let expected_size = crate::stdio_transport::v3_serialize_call_tool_result(
                &build_tool_result_for_surface_v3(
                    *revision,
                    crate::prove_call_path::PUBLIC_VERIFY_TOOL_NAME,
                    &oversized_public,
                    V3SurfaceSet::WithProof,
                )
                .unwrap(),
            )
            .unwrap()
            .len();
            let fallback = build_proof_tool_result_v3(*revision, &oversized).unwrap();
            let fallback_bytes =
                crate::stdio_transport::v3_serialize_call_tool_result(&fallback).unwrap();
            assert!(fallback_bytes.len() <= PROOF_TOOL_RESULT_MAX_BYTES_V3);
            let fallback_root =
                serde_json::from_str::<Value>(fallback["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            assert_eq!(fallback_root["kind"], "budget_exceeded");
            assert_eq!(fallback_root["required_complete_size"], expected_size);
            assert_eq!(fallback_root["domain"], "call-path/v1");
            assert_eq!(fallback_root["runtime_execution_proven"], false);
            if revision.profile().structured_content {
                assert_eq!(fallback["structuredContent"], fallback_root);
            }
        }

        let mut fallback_too_large = actual_projected_root("A calls B();\n".to_owned());
        fallback_too_large["core_publication"]["project_id"] = json!("p".repeat(70_000));
        let error = build_proof_tool_result_v3(McpRevisionV3::November2024, &fallback_too_large)
            .expect_err("oversized fallback must fail internally");
        assert!(matches!(
            error,
            StdioV3InternalError::ResultExceedsBudget { .. }
        ));
        assert_eq!(
            jsonrpc_internal_error_v3(json!(99), &error).pointer("/error/code"),
            Some(&json!(-32603))
        );
    }

    fn proof_root(disposition_kind: &str) -> Value {
        match disposition_kind {
            "proven" => actual_projected_root("x".to_owned()),
            "unknown" | "unavailable" => {
                let (contract, hashes, rendering) = validated_test_contract();
                let integration = check_built_call_path_integration(
                    &contract,
                    &hashes,
                    &rendering,
                    BuiltCallPathFacts {
                        publication: InternalCorePublicationIdentity {
                            project_id: "project".to_owned(),
                            generation_id: "generation".to_owned(),
                            run_id: "run".to_owned(),
                        },
                        facts: Vec::new(),
                        receipts: Vec::new(),
                        gaps: (disposition_kind == "unknown")
                            .then_some(FactBuildGap::DirectCallMissing { step_index: 0 })
                            .into_iter()
                            .collect(),
                        unavailable: (disposition_kind == "unavailable")
                            .then_some(UnavailableReason::PublicationPinMismatch)
                            .into_iter()
                            .collect(),
                    },
                )
                .expect("canonical uncertainty integration");
                let InternalProjection::Complete { root, .. } =
                    project_internal_call_path_result(&integration)
                        .expect("canonical uncertainty projection")
                else {
                    panic!("uncertainty fixture remains a complete projection")
                };
                codestory_runtime::proof_qualification_support::validate_compact_projection(&root)
                    .unwrap_or_else(|error| {
                        panic!("canonical {disposition_kind} projection must validate: {error}")
                    });
                root
            }
            other => panic!("unsupported fixture disposition {other}"),
        }
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
    fn legacy_batch_rejects_hostile_envelopes_and_preserves_valid_error_ids() {
        let frame = json!([
            {"jsonrpc":"2.0","id":"unknown-field","method":"tools/list","extra":true},
            {"jsonrpc":"2.0","id":17,"method":"tools/call","params":null},
            {"jsonrpc":"1.0","id":0,"method":"tools/list"},
            {"jsonrpc":"2.0","id":"missing-method"},
            {"jsonrpc":"2.0","id":false,"method":"tools/list"},
            ["nested-batch"],
            {"jsonrpc":"2.0","id":"array-params","method":"tools/call","params":[]},
            {"jsonrpc":"2.0","method":"notifications/initialized","params":{}},
            {"jsonrpc":"2.0","id":"object-params","method":"tools/call","params":{}}
        ]);
        for revision in [McpRevisionV3::November2024, McpRevisionV3::March2025] {
            let mut called = Vec::new();
            let response = process_jsonrpc_frame_v3(revision, &frame, |request| {
                called.push(request["method"].as_str().unwrap().to_string());
                json!({"jsonrpc":"2.0","id":request.get("id").cloned().unwrap_or(Value::Null),"result":{}})
            });
            assert_eq!(
                called,
                ["tools/call", "notifications/initialized", "tools/call"],
                "only closed, valid JSON-RPC envelopes may reach dispatch"
            );
            let FrameResponseV3::Batch(responses) = response else {
                panic!("legacy hostile batch must return every non-notification response")
            };
            assert_eq!(responses.len(), 8);
            let expected_ids = json!([
                "unknown-field",
                17,
                0,
                "missing-method",
                null,
                null,
                "array-params",
                "object-params"
            ]);
            assert_eq!(
                responses
                    .iter()
                    .map(|response| response["id"].clone())
                    .collect::<Vec<_>>(),
                expected_ids.as_array().unwrap().clone()
            );
            for response in &responses[..6] {
                assert_eq!(response.pointer("/error/code"), Some(&json!(-32600)));
                assert_eq!(
                    response.pointer("/error/message"),
                    Some(&json!("Invalid Request"))
                );
            }
            assert!(responses[6].get("result").is_some());
            assert!(responses[7].get("result").is_some());
        }
    }

    #[test]
    fn result_profiles_are_revision_native_and_keep_typed_uncertainty_successful() {
        for disposition in ["unknown", "unavailable"] {
            let root = proof_root(disposition);
            let public = crate::prove_call_path::project_public_verification_result(root.clone())
                .expect("project public verification result");
            for revision in McpRevisionV3::all() {
                let result = build_proof_tool_result_v3(*revision, &root).unwrap_or_else(|error| {
                    panic!("{disposition} {revision:?} tool result: {error:?}")
                });
                assert_eq!(result["isError"], false);
                let text = result
                    .pointer("/content/0/text")
                    .and_then(Value::as_str)
                    .unwrap();
                assert_eq!(serde_json::from_str::<Value>(text).unwrap(), public);
                if revision.profile().structured_content {
                    assert_eq!(result["structuredContent"], public);
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

    #[cfg(feature = "proof-qualification-support")]
    #[test]
    fn post_budget_validation_suppresses_invalid_payload_and_whole_result_falls_back() {
        let mut invalid = proof_root("unknown");
        invalid["undeclared"] = json!(true);
        let error = build_proof_tool_result_v3(McpRevisionV3::June2025, &invalid)
            .expect_err("undeclared output must fail closed");
        assert!(
            matches!(
                error,
                StdioV3InternalError::OutputSchemaViolation
                    | StdioV3InternalError::InvalidProjection(_)
            ),
            "undeclared proof fields must fail closed: {error:?}"
        );
        let response = jsonrpc_internal_error_v3(json!(7), &error);
        assert_eq!(response.pointer("/error/code"), Some(&json!(-32603)));
        assert!(response.pointer("/error/data/structuredContent").is_none());

        let mut oversized = proof_root("proven");
        set_receipt_line_text(&mut oversized, "\\\"é".repeat(24_000));
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
    fn every_activation_capable_tool_accepts_preparing_through_the_native_builder() {
        let preparing = json!({
            "kind": "preparing",
            "state": "preparing",
            "retry_after_ms": 250,
            "operation": {"stage":"publication"}
        });
        let activation_tools = crate::stdio_catalog::v3_tool_source_json()
            .into_iter()
            .filter(|tool| tool.pointer("/safety/activatesProject") == Some(&json!(true)))
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert!(activation_tools.iter().any(|name| name == "ground"));

        for revision in [McpRevisionV3::June2025, McpRevisionV3::November2025] {
            for tool_name in &activation_tools {
                let result = build_tool_result_v3(revision, tool_name, &preparing)
                    .unwrap_or_else(|error| panic!("{revision:?} {tool_name}: {error:?}"));
                assert_eq!(result["isError"], false);
                assert_eq!(result["structuredContent"], preparing);
                assert_eq!(
                    serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap())
                        .unwrap(),
                    preparing
                );
            }
            assert_eq!(
                build_tool_result_v3(revision, "status", &preparing),
                Err(StdioV3InternalError::OutputSchemaViolation),
                "observational tools must not gain a preparing branch"
            );
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
                    project_identity: "project-1".into(),
                    core_generation: "core-1".into(),
                    core_run: "run-1".into(),
                    retrieval_generation: Some("retrieval-1".into()),
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
            "identity": {
                "packet_id":"b96ac0cc-e552-4c35-a0ba-c83b9ead67de",
                "request_id":"request-1",
                "question_sha256":"a".repeat(64)
            },
            "publication": {
                "core":{"project_id":"project-1","generation_id":"core-1","run_id":"run-1"},
                "retrieval":null
            },
            "status":"available",
            "retrieval":{"state":"full","generation_id":null},
            "evidence":[],
            "gaps":[],
            "continuation":null,
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
        assert_eq!(grant.wall_expiry_epoch_ms, 99_000);
        assert_eq!(
            projection.pointer("/diagnostics/reference/wall_expiry_epoch_ms"),
            Some(&json!(99_000))
        );

        for revision in McpRevisionV3::all() {
            let result = build_tool_result_v3(*revision, "packet", &projection)
                .expect("revision-native packet result");
            let mirrored = result
                .pointer("/content/0/text")
                .and_then(Value::as_str)
                .unwrap();
            let mirrored = serde_json::from_str::<Value>(mirrored).unwrap();
            assert_eq!(mirrored, projection);
            assert_eq!(
                mirrored.pointer("/diagnostics/reference/wall_expiry_epoch_ms"),
                Some(&json!(99_000))
            );
            if revision.profile().structured_content {
                assert_eq!(
                    result.pointer("/structuredContent/diagnostics/reference/wall_expiry_epoch_ms"),
                    Some(&json!(99_000))
                );
            }
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

    #[cfg(feature = "proof-qualification-support")]
    #[test]
    fn measurement_seam_owns_exact_builder_bytes_for_every_revision() {
        let root = proof_root("proven");
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
