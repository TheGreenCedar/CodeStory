use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use codestory_runtime::proof_qualification_support as proof;

use crate::call_path_grammar::{CALL_PATH_GRAMMAR_HEADER, parse_call_path_document};

pub(crate) const PUBLIC_VERIFY_TOOL_NAME: &str = "verify_indexed_direct_calls";
/// The public domain and the grammar version are the same identity: a result
/// names the grammar its contract was written in.
pub(crate) const PUBLIC_CALL_PATH_DOMAIN: &str = CALL_PATH_GRAMMAR_HEADER;
pub(crate) const PROVE_CALL_PATH_INPUT_MAX_BYTES: usize = 8 * 1024;
/// The one public request field: the `call-path/v1` document itself.
pub(crate) const CALL_PATH_ARGUMENT: &str = "call_path";

pub(crate) fn is_proof_tool_name(name: &str) -> bool {
    name == PUBLIC_VERIFY_TOOL_NAME
}

pub(crate) fn project_public_verification_result(internal: Value) -> Result<Value, String> {
    let object = internal
        .as_object()
        .ok_or_else(|| "proof projection root must be an object".to_owned())?;
    let mut public = object.clone();
    public.insert(
        "domain".to_owned(),
        Value::String(PUBLIC_CALL_PATH_DOMAIN.to_owned()),
    );
    let translation_status = public
        .remove("contract_interpretation")
        .unwrap_or_else(|| Value::String(proof::CONTRACT_INTERPRETATION.to_owned()));
    public.insert("translation_status".to_owned(), translation_status);
    rewrite_forbidden_public_absence(&mut public)?;
    public.insert(
        "graph_disposition".to_owned(),
        Value::String(
            public
                .get("disposition")
                .map(graph_disposition_from_disposition)
                .unwrap_or("unknown")
                .to_owned(),
        ),
    );
    public.insert("runtime_execution_proven".to_owned(), Value::Bool(false));
    attach_proof_provenance_capability(&mut public);
    apply_public_compact_budget(Value::Object(public))
}

fn apply_public_compact_budget(root: Value) -> Result<Value, String> {
    let serialized = serde_json::to_vec(&root)
        .map_err(|error| format!("serialize public verification result: {error}"))?;
    if serialized.len() <= proof::COMPACT_PROOF_MAX_BYTES {
        return Ok(root);
    }
    let object = root
        .as_object()
        .ok_or_else(|| "proof projection root must be an object".to_owned())?;
    let contract_digest = object.get("contract_digest").cloned().unwrap_or(json!(""));
    let mut compact = json!({
        "kind": "budget_exceeded",
        "schema_version": object.get("schema_version").cloned().unwrap_or(json!(1)),
        "domain": object.get("domain").cloned().unwrap_or(json!(PUBLIC_CALL_PATH_DOMAIN)),
        "translation_status": object.get("translation_status").cloned().unwrap_or(json!(proof::CONTRACT_INTERPRETATION)),
        "graph_disposition": "unknown",
        "runtime_execution_proven": false,
        "guard_version": object.get("guard_version").cloned().unwrap_or(json!("clause_guard_v1")),
        "source_text_sha256": object.get("source_text_sha256").cloned().unwrap_or(json!("")),
        "contract_digest": contract_digest.clone(),
        "core_publication": object.get("core_publication").cloned().unwrap_or(json!({})),
        "disposition": {
            "kind": "unknown",
            "contract_digest": contract_digest,
            "gaps": [{"kind": "output_budget_exceeded"}]
        },
        "cap_bytes": proof::COMPACT_PROOF_MAX_BYTES,
        "required_complete_size": serialized.len(),
    });
    if let Some(compact_object) = compact.as_object_mut() {
        if let Some(provenance) = object.get("provenance").cloned() {
            compact_object.insert("provenance".to_owned(), provenance);
        }
        attach_proof_provenance_capability(compact_object);
    }
    let compact_bytes = serde_json::to_vec(&compact)
        .map_err(|error| format!("serialize public verification budget envelope: {error}"))?;
    if compact_bytes.len() > proof::COMPACT_PROOF_MAX_BYTES {
        return Err(format!(
            "public verification result exceeds {} bytes even after budget projection ({} bytes)",
            proof::COMPACT_PROOF_MAX_BYTES,
            compact_bytes.len()
        ));
    }
    Ok(compact)
}

fn rewrite_forbidden_public_absence(
    public: &mut serde_json::Map<String, Value>,
) -> Result<(), String> {
    let Some(disposition) = public.get("disposition").cloned() else {
        return Ok(());
    };
    let is_certified_absence = disposition.get("kind").and_then(Value::as_str)
        == Some("contract_refuted")
        && disposition
            .pointer("/refutation/kind")
            .and_then(Value::as_str)
            == Some("certified_absence");
    if !is_certified_absence {
        return Ok(());
    }
    let contract_digest = disposition
        .get("contract_digest")
        .cloned()
        .ok_or_else(|| "certified_absence refutation missing contract_digest".to_owned())?;
    // Public call-path/v1 must not project certified absence as refutation.
    public.insert(
        "disposition".to_owned(),
        json!({
            "kind": "unavailable",
            "contract_digest": contract_digest,
            "reasons": ["proof_facts_unavailable"]
        }),
    );
    if let Some(steps) = public.get_mut("steps").and_then(Value::as_array_mut) {
        for step in steps {
            if step.get("status").and_then(Value::as_str) == Some("certified_absence") {
                step["status"] = json!("unavailable");
            }
        }
    }
    Ok(())
}

fn graph_disposition_from_disposition(disposition: &Value) -> &'static str {
    match disposition.get("kind").and_then(Value::as_str) {
        Some("contract_proven") => "proven",
        Some("contract_refuted") => {
            if disposition
                .pointer("/refutation/kind")
                .and_then(Value::as_str)
                == Some("certified_absence")
            {
                "unknown"
            } else {
                "refuted"
            }
        }
        _ => "unknown",
    }
}

/// There is no proof-provenance artifact registry and no resource route that
/// could serve one, so the only honest report is that the capability is
/// unavailable. Claiming availability with a zero-byte reference would advertise
/// a URI no caller can read. Tracked by issue #2104.
fn attach_proof_provenance_capability(public: &mut serde_json::Map<String, Value>) {
    if public.contains_key("provenance") {
        return;
    }
    public.insert(
        "provenance".to_owned(),
        json!({ "availability": "unavailable" }),
    );
}

/// Reads the single public request field and parses the grammar. The internal
/// contract, its clause anchors, and its selectors are produced here; no caller
/// can supply a classification or a pinned internal node identity.
pub(crate) fn parse_request(
    arguments: Value,
) -> Result<proof::UnvalidatedCallPathContract, String> {
    let object = arguments
        .as_object()
        .ok_or_else(|| format!("{PUBLIC_VERIFY_TOOL_NAME} arguments must be an object"))?;
    let unexpected = object
        .keys()
        .filter(|key| key.as_str() != CALL_PATH_ARGUMENT)
        .cloned()
        .collect::<Vec<_>>();
    if !unexpected.is_empty() {
        return Err(format!(
            "{PUBLIC_VERIFY_TOOL_NAME} accepts only `{CALL_PATH_ARGUMENT}`; unexpected {}",
            unexpected.join(", ")
        ));
    }
    let document = object
        .get(CALL_PATH_ARGUMENT)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!("{PUBLIC_VERIFY_TOOL_NAME} requires `{CALL_PATH_ARGUMENT}` as a {CALL_PATH_GRAMMAR_HEADER} text document")
        })?;
    if document.len() > PROVE_CALL_PATH_INPUT_MAX_BYTES {
        return Err(format!(
            "{PUBLIC_VERIFY_TOOL_NAME} `{CALL_PATH_ARGUMENT}` exceeds the {PROVE_CALL_PATH_INPUT_MAX_BYTES} byte input limit"
        ));
    }
    parse_call_path_document(document).map_err(|error| error.message)
}

pub(crate) fn validate_request(
    contract: proof::UnvalidatedCallPathContract,
) -> Result<proof::ValidationOutcome, String> {
    proof::validate_contract(contract).map_err(|error| format!("{error:?}"))
}

pub(crate) fn projection_root(
    operation: &codestory_runtime::PublicOperation<
        proof::ObservedIntegratedProjectedCallPathResult,
    >,
) -> Result<Value, String> {
    let result = operation
        .value
        .result
        .as_ref()
        .map_err(|error| error.message.clone())?;
    let root = match &result.projection {
        proof::InternalProjection::Complete { root, .. }
        | proof::InternalProjection::BudgetExceeded { root, .. } => root,
    };
    Ok(root.clone())
}

pub(crate) fn internal_projection_root(projection: &proof::InternalProjection) -> Value {
    match projection {
        proof::InternalProjection::Complete { root, .. }
        | proof::InternalProjection::BudgetExceeded { root, .. } => root.clone(),
    }
}

/// Reads a `call-path/v1` document from a file or stdin under the same 8 KiB
/// request cap the MCP surface applies, so both transports accept exactly the
/// same contracts.
pub(crate) fn read_bounded_call_path(path: &Path) -> Result<String> {
    let bytes = if path.as_os_str() == "-" {
        read_bounded(std::io::stdin().lock(), "stdin")?
    } else {
        let file = std::fs::File::open(path)
            .with_context(|| format!("open call path document {}", path.display()))?;
        read_bounded(file, &format!("call path document {}", path.display()))?
    };
    String::from_utf8(bytes).context("call path document must be UTF-8 text")
}

fn read_bounded(reader: impl Read, source: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(PROVE_CALL_PATH_INPUT_MAX_BYTES.min(8 * 1024));
    reader
        .take((PROVE_CALL_PATH_INPUT_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {source}"))?;
    if bytes.len() > PROVE_CALL_PATH_INPUT_MAX_BYTES {
        bail!(
            "call path document exceeds the {} byte input limit",
            PROVE_CALL_PATH_INPUT_MAX_BYTES
        );
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn internal_fixture(disposition_kind: &str) -> Value {
        json!({
            "kind": "complete",
            "schema_version": 1,
            "domain": "indexed_source_call_path_v1",
            "contract_interpretation": "host_supplied",
            "guard_version": "clause_guard_v1",
            "source_text_sha256": "a".repeat(64),
            "contract_digest": "b".repeat(64),
            "core_publication": {"project_id":"p","generation_id":"g","run_id":"r"},
            "disposition": {"kind": disposition_kind, "contract_digest":"b".repeat(64), "gaps":[{"kind":"direct_call_missing","step_index":0}], "connected_receipts":[]},
            "identities": {"files":[],"symbols":[],"provenance_profiles":[],"evidence":[]},
            "spec": {"start":{"kind":"canonical_id","canonical_id":"A"},"steps":[{"relation":"direct_outgoing_call","target":{"kind":"canonical_id","canonical_id":"B"}}],"prohibit_traversal_through":[],"exclude_from_projection":[]},
            "clauses": [{"start":0,"end":1,"clause_id":"c","quote":"x","classification":"resolved_material","fields":[{"kind":"start"}],"reason":null,"non_material_kind":null}],
            "steps": [{"step_index":0,"status":"unknown","receipt":null}],
            "receipts": []
        })
    }

    #[test]
    fn public_projection_exposes_phase6_verification_fields() {
        for (internal_kind, graph_disposition) in [
            ("contract_proven", "proven"),
            ("contract_refuted", "refuted"),
            ("unknown", "unknown"),
            ("unavailable", "unknown"),
        ] {
            let public = project_public_verification_result(internal_fixture(internal_kind))
                .expect("project public verification result");
            assert_eq!(public["domain"], "call-path/v1");
            assert_eq!(public["translation_status"], "host_supplied");
            assert_eq!(public["graph_disposition"], graph_disposition);
            assert_eq!(public["runtime_execution_proven"], false);
            assert!(public.get("contract_interpretation").is_none());
            assert_eq!(
                public["provenance"],
                json!({"availability": "unavailable"}),
                "there is no provenance registry, so no reference may be advertised"
            );
        }
    }

    #[test]
    fn public_projection_rejects_certified_absence_as_refutation() {
        let mut internal = internal_fixture("contract_refuted");
        internal["disposition"] = json!({
            "kind":"contract_refuted",
            "contract_digest":"b".repeat(64),
            "refutation":{
                "kind":"certified_absence",
                "step_index":0,
                "extractor_capability_receipt_id":"extractor:fixture",
                "untruncated_enumeration_receipt_id":"enumeration:fixture",
                "connected_receipts":[]
            }
        });
        internal["steps"] = json!([{"step_index":0,"status":"certified_absence","receipt":null}]);
        let public = project_public_verification_result(internal)
            .expect("project public verification result");
        assert_eq!(public["graph_disposition"], "unknown");
        assert_eq!(
            public.pointer("/disposition/kind"),
            Some(&json!("unavailable"))
        );
        assert_ne!(
            public.pointer("/disposition/refutation/kind"),
            Some(&json!("certified_absence"))
        );
        assert_eq!(public["steps"][0]["status"], "unavailable");
    }

    #[test]
    fn the_retired_proof_tool_alias_is_not_recognized() {
        assert!(is_proof_tool_name(PUBLIC_VERIFY_TOOL_NAME));
        assert!(!is_proof_tool_name("prove_call_path"));
        assert!(!is_proof_tool_name("packet"));
    }

    #[test]
    fn proof_input_cap_is_eight_kib() {
        assert_eq!(PROVE_CALL_PATH_INPUT_MAX_BYTES, 8 * 1024);
    }

    #[test]
    fn the_request_accepts_only_the_call_path_document() {
        let document =
            "call-path/v1\nfrom symbol \"crate::Alpha\"\ndirect-call symbol \"crate::Beta\"\n"
                .to_owned();
        parse_request(json!({ "call_path": document.clone() })).expect("a grammar document parses");

        for hostile in [
            json!({}),
            json!({ "call_path": 1 }),
            json!({"call_path": document.clone(), "source_text": "x"}),
            json!({"source_text": document.clone(), "clauses": [], "spec": {}}),
        ] {
            assert!(
                parse_request(hostile.clone()).is_err(),
                "the public request must reject {hostile}"
            );
        }
    }

    #[test]
    fn public_budget_overflow_emits_unknown_output_budget_exceeded() {
        let mut internal = internal_fixture("contract_proven");
        internal["clauses"] = json!([{
            "start": 0,
            "end": 1,
            "clause_id": "c",
            "quote": "x".repeat(5 * 1024),
            "classification": "resolved_material",
            "fields": [{"kind": "start"}],
            "reason": null,
            "non_material_kind": null
        }]);
        let public = project_public_verification_result(internal)
            .expect("oversized public verification must emit a typed budget envelope");
        assert_eq!(public["kind"], "budget_exceeded");
        assert_eq!(public["graph_disposition"], "unknown");
        assert_eq!(public["guard_version"], "clause_guard_v1");
        assert_eq!(
            public["provenance"],
            json!({"availability": "unavailable"})
        );
        assert_eq!(public.pointer("/disposition/kind"), Some(&json!("unknown")));
        assert_eq!(
            public.pointer("/disposition/gaps/0/kind"),
            Some(&json!("output_budget_exceeded"))
        );
        assert!(public.pointer("/disposition/reasons").is_none());
        let bytes = serde_json::to_vec(&public).expect("serialize budget envelope");
        assert!(bytes.len() <= proof::COMPACT_PROOF_MAX_BYTES);
    }

    #[test]
    fn parse_request_rejects_multibyte_documents_over_eight_kib() {
        let oversized = format!(
            "call-path/v1\nfrom symbol \"{}\"\ndirect-call symbol \"crate::Beta\"\n",
            "字".repeat(3 * 1024)
        );
        assert!(oversized.len() > PROVE_CALL_PATH_INPUT_MAX_BYTES);
        assert!(oversized.chars().count() <= PROVE_CALL_PATH_INPUT_MAX_BYTES);
        let error = parse_request(json!({ "call_path": oversized }))
            .expect_err("UTF-8 byte length, not character count, is the input cap");
        assert!(error.contains("byte"), "{error}");
    }

    #[test]
    fn a_pinned_internal_node_is_not_a_public_selector() {
        let error = parse_request(json!({
            "call_path": "call-path/v1\nfrom symbol \"{\\\"kind\\\":\\\"pinned_node\\\",\\\"node_id\\\":\\\"7\\\"}\"\ndirect-call symbol \"crate::Beta\"\n"
        }))
        .expect_err("internal node identities never cross the public surface");
        assert!(error.contains("start"), "{error}");
    }
}
