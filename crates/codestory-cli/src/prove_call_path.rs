use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

use codestory_runtime::proof_qualification_support as proof;

pub(crate) const PUBLIC_VERIFY_TOOL_NAME: &str = "verify_indexed_direct_calls";
pub(crate) const PROVE_CALL_PATH_INPUT_MAX_BYTES: usize = 8 * 1024;
/// The one public request field: the `call-path/v1` document itself.
pub(crate) const CALL_PATH_ARGUMENT: &str = "call_path";

pub(crate) fn is_proof_tool_name(name: &str) -> bool {
    name == PUBLIC_VERIFY_TOOL_NAME
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
            format!("{PUBLIC_VERIFY_TOOL_NAME} requires `{CALL_PATH_ARGUMENT}` as a call-path/v1 text document")
        })?;
    if document.len() > PROVE_CALL_PATH_INPUT_MAX_BYTES {
        return Err(format!(
            "{PUBLIC_VERIFY_TOOL_NAME} `{CALL_PATH_ARGUMENT}` exceeds the {PROVE_CALL_PATH_INPUT_MAX_BYTES} byte input limit"
        ));
    }
    codestory_runtime::proof_qualification_support::parse_public_call_path_document(document)
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
