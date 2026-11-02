use super::*;
use codestory_contracts::source_expression::*;

#[test]
fn analysis_result_caps_actual_profile_escaping_and_mirrors_and_fallback() {
    // This is a transport DTO fixture, not native source-identity evidence.
    let mut response = SourceExpressionResponse::unavailable(AnalysisDecline::InvalidCriterion);
    response.status = AnalysisStatus::Partial;
    response.decline = None;
    response.gaps = vec![AnalysisGap::UnsupportedConstruct];
    response.segments.push(AnalysisSegment {
        start_byte: 0,
        end_byte: 2800,
        start_line: 1,
        end_line: 1,
        body_sha256: "0".repeat(64),
        source: "\"".repeat(2800),
    });
    let mut refused = 0;
    let mut retained = 0;
    for revision in [
        crate::stdio_v3::McpRevisionV3::November2024,
        crate::stdio_v3::McpRevisionV3::March2025,
        crate::stdio_v3::McpRevisionV3::June2025,
        crate::stdio_v3::McpRevisionV3::November2025,
        crate::stdio_v3::McpRevisionV3::June2025JsonText,
        crate::stdio_v3::McpRevisionV3::November2025JsonText,
    ] {
        let result = build_source_expression_result_v3(revision, &response).unwrap();
        assert!(v3_serialize_call_tool_result(&result).unwrap().len() <= MAX_RESULT_BYTES);
        let root: serde_json::Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        if root["result"]["status"] == "unavailable" {
            refused += 1;
            assert_eq!(root["result"]["decline"], "result_budget_exceeded");
            assert_eq!(root["result"]["segments"], serde_json::json!([]));
        } else {
            retained += 1;
            assert_eq!(
                root["result"]["segments"][0]["source"],
                response.segments[0].source
            );
        }
        if let Some(structured) = result.get("structuredContent") {
            assert_eq!(structured, &root);
        }
    }
    assert!(
        refused > 0 && retained > 0,
        "profiles must measure their own complete result, not only DTO/source bytes"
    );
    response.segments[0].source = "\\\"".repeat(30_000);
    for revision in crate::stdio_v3::McpRevisionV3::all() {
        let result = build_source_expression_result_v3(*revision, &response).unwrap();
        assert!(v3_serialize_call_tool_result(&result).unwrap().len() <= MAX_RESULT_BYTES);
        let root: serde_json::Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(root["result"]["decline"], "result_budget_exceeded");
    }
}
