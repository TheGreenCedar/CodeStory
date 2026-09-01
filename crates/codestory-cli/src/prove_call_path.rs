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
    Ok(Value::Object(public))
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
pub(crate) fn parse_request(arguments: Value) -> Result<proof::UnvalidatedCallPathContract, String> {
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
            "contract_interpretation": "parser_derived",
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
            assert_eq!(public["translation_status"], "parser_derived");
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
            "call-path/v1\nstart: crate::Alpha\nstep 1: direct call -> crate::Beta\n".to_owned();
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
    fn a_pinned_internal_node_is_not_a_public_selector() {
        let error = parse_request(json!({
            "call_path": "call-path/v1\nstart: {\"kind\":\"pinned_node\",\"node_id\":\"7\"}\nstep 1: direct call -> crate::Beta\n"
        }))
        .expect_err("internal node identities never cross the public surface");
        assert!(error.contains("start"), "{error}");
    }
}
