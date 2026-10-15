// Independent anonymous controls. Include inside stdio_transport::root_symbols
// with #[cfg(test)] #[path = "..."] mod independent_tests;
use super::*;

fn publication() -> serde_json::Value {
    serde_json::json!({"generation":"anonymous-g1","run_id":"anonymous-r1"})
}
fn row(id: usize, label: String) -> serde_json::Value {
    serde_json::json!({"id":id,"label":label,"kind":"FUNCTION","file_path":"anonymous.rs","has_children":false})
}
fn body(response: &serde_json::Value) -> serde_json::Value {
    serde_json::from_str(response["result"]["contents"][0]["text"].as_str().unwrap()).unwrap()
}
fn render_fixture(
    request: &PageRequest,
    id: &serde_json::Value,
    total: usize,
    rows: Vec<serde_json::Value>,
) -> Result<serde_json::Value> {
    render(
        request,
        id,
        "codestory://symbols/root?project=%2Fanonymous",
        "anonymous-project",
        publication(),
        &"a".repeat(64),
        total,
        rows,
    )
}
fn next_request(response: &serde_json::Value) -> PageRequest {
    let value = body(response);
    let uri = value["continuation_uri"].as_str().unwrap();
    let (project, request) = PageRequest::parse(uri.split_once('?').map(|(_, q)| q)).unwrap();
    assert_eq!(project.as_deref(), Some("/anonymous"));
    request
}
fn wire_bytes(id: &serde_json::Value, response: &serde_json::Value) -> usize {
    serde_json::to_vec(&stdio_jsonrpc_from_legacy(id.clone(), response.clone()))
        .unwrap()
        .len()
}

#[test]
fn independent_root_selectors_are_strict_and_root_only() {
    let (project, default) = PageRequest::parse(None).unwrap();
    assert!(project.is_none());
    assert_eq!(default.limit, 50);
    assert!(default.cursor.is_none());
    for limit in [1, 50, 200] {
        assert_eq!(
            PageRequest::parse(Some(&format!("limit={limit}")))
                .unwrap()
                .1
                .limit,
            limit
        );
    }
    for query in [
        "",
        "limit=0",
        "limit=201",
        "limit=01",
        "limit=+1",
        "limit=-1",
        "limit=",
        "limit=1=2",
        "limit=184467440737095516160",
        "limit=1&limit=2",
        "limit=1&",
        "limit=1&&project=x",
        "unknown=x",
        "project=",
        "project=x&project=y",
        "project=%2fanonymous",
        "project=%61",
        "project=/anonymous",
        "project=x?y",
    ] {
        assert!(
            PageRequest::parse(Some(query)).is_err(),
            "accepted {query:?}"
        );
    }
    for uri in [
        "codestory://project?project=x&limit=1",
        "codestory://status?project=x&cursor=x",
        "codestory://symbol/1?project=x&limit=1",
        "codestory://agent-guide?limit=1",
    ] {
        assert!(
            StdioResource::parse(uri).is_err(),
            "accepted non-root selector {uri}"
        );
    }
}

#[test]
fn independent_root_cursor_grammar_and_project_roundtrip() {
    let good = format!("rs1.{}.1", "a".repeat(64));
    assert_eq!(
        offset(&PageRequest {
            limit: 1,
            cursor: Some(good.clone())
        })
        .unwrap(),
        1
    );
    for cursor in [
        format!("r1.{}.1", "a".repeat(64)),
        format!("rs1.{}.0", "a".repeat(64)),
        format!("rs1.{}.01", "a".repeat(64)),
        format!("rs1.{}.+1", "a".repeat(64)),
        format!("rs1.{}.-1", "a".repeat(64)),
        format!("rs1.{}.1.", "a".repeat(64)),
        format!("rs1.{}.1", "A".repeat(64)),
        format!("rs1.{}.1", "g".repeat(64)),
        format!("rs1.{}.1", "a".repeat(63)),
        format!("rs1.{}.184467440737095516160", "a".repeat(64)),
        format!("{good} "),
    ] {
        assert!(parse_cursor(&cursor).is_err(), "accepted {cursor}");
    }
    let project = "/anonymous space/%/café";
    let query = format!(
        "cursor={good}&limit=7&project={}",
        strict_uri_component_encode(project)
    );
    let (decoded, request) = PageRequest::parse(Some(&query)).unwrap();
    assert_eq!(decoded.as_deref(), Some(project));
    let base = format!(
        "codestory://symbols/root?project={}",
        strict_uri_component_encode(project)
    );
    let uri = request.uri(&base);
    let (again, request2) = PageRequest::parse(uri.split_once('?').map(|(_, q)| q)).unwrap();
    assert_eq!(again, decoded);
    assert_eq!(request2, request);
    assert!(PageRequest::parse(Some(&format!("cursor={good}&cursor={good}"))).is_err());
    assert!(PageRequest::parse(Some(&format!("cursor=%72{}", &good[1..]))).is_err());
}

#[test]
fn independent_root_empty_and_incomplete_inputs_do_not_advance() {
    let req = PageRequest::default();
    let id = serde_json::json!(null);
    let response = render_fixture(&req, &id, 0, vec![]).unwrap();
    let value = body(&response);
    assert_eq!(value["symbols"], serde_json::json!([]));
    for key in [
        "total_count",
        "start_offset",
        "returned_count",
        "remaining_count",
    ] {
        assert_eq!(value[key], 0);
    }
    assert_eq!(value["truncated"], false);
    assert!(value["continuation_uri"].is_null());
    assert!(wire_bytes(&id, &response) <= MAX_PAGE_BYTES);
    assert!(render_fixture(&req, &id, 1, vec![]).is_err());
    assert!(
        render_fixture(
            &PageRequest {
                limit: 1,
                cursor: None
            },
            &id,
            2,
            vec![row(1, "one".into()), row(2, "two".into())]
        )
        .is_err()
    );
    assert!(render_fixture(&req, &id, 0, vec![row(1, "one".into())]).is_err());
}

#[test]
fn independent_root_binding_rejects_foreign_stale_and_content_drift() {
    let id = serde_json::json!("anonymous-id");
    let first = render_fixture(
        &PageRequest {
            limit: 1,
            cursor: None,
        },
        &id,
        2,
        vec![row(0, "zero".into())],
    )
    .unwrap();
    let request = next_request(&first);
    let accepted = render_fixture(&request, &id, 2, vec![row(1, "one".into())]).unwrap();
    assert_eq!(body(&accepted)["start_offset"], 1);
    for (project, pubvalue, inventory) in [
        ("foreign-project", publication(), "a".repeat(64)),
        (
            "anonymous-project",
            serde_json::json!({"generation":"anonymous-g2","run_id":"anonymous-r1"}),
            "a".repeat(64),
        ),
        (
            "anonymous-project",
            serde_json::json!({"generation":"anonymous-g1","run_id":"anonymous-r2"}),
            "a".repeat(64),
        ),
        ("anonymous-project", publication(), "b".repeat(64)),
    ] {
        let error = render(
            &request,
            &id,
            "codestory://symbols/root?project=%2Fanonymous",
            project,
            pubvalue,
            &inventory,
            2,
            vec![row(1, "one".into())],
        )
        .unwrap_err();
        assert!(error.to_string().contains("root_symbols_cursor_stale"));
    }
    let digest = parse_cursor(request.cursor.as_deref().unwrap()).unwrap().0;
    let beyond = PageRequest {
        limit: 1,
        cursor: Some(format!("rs1.{digest}.3")),
    };
    assert!(
        render_fixture(&beyond, &id, 2, vec![])
            .unwrap_err()
            .to_string()
            .contains("root_symbols_page_incomplete")
    );
}

#[test]
fn independent_root_wire_escaping_shrinks_without_skipping() {
    let request = PageRequest {
        limit: 2,
        cursor: None,
    };
    let id = serde_json::json!("ordinary");
    let rows = vec![row(0, "small".into()), row(1, "\"".repeat(2600))];
    // Plain inner JSON fits; the enclosing resource text needs a second escape pass.
    let plain = serde_json::to_vec(&serde_json::json!({"symbols":rows.clone()})).unwrap();
    assert!(plain.len() < MAX_PAGE_BYTES);
    let response = render_fixture(&request, &id, 2, rows).unwrap();
    let value = body(&response);
    assert_eq!(value["returned_count"], 1);
    assert_eq!(value["symbols"][0]["id"], 0);
    assert_eq!(value["remaining_count"], 1);
    assert_eq!(value["truncated"], true);
    assert_eq!(offset(&next_request(&response)).unwrap(), 1);
    assert!(wire_bytes(&id, &response) <= MAX_PAGE_BYTES);
}

#[test]
fn independent_root_actual_id_exact_boundary_and_refusal() {
    let request = PageRequest {
        limit: 1,
        cursor: None,
    };
    let rows = vec![row(0, "one".into())];
    let response = render_fixture(&request, &serde_json::json!(""), 1, rows.clone()).unwrap();
    let fixed = wire_bytes(&serde_json::json!(""), &response);
    assert!(fixed < MAX_PAGE_BYTES);
    let exact_id = serde_json::json!("i".repeat(MAX_PAGE_BYTES - fixed));
    let exact = render_fixture(&request, &exact_id, 1, rows.clone()).unwrap();
    assert_eq!(wire_bytes(&exact_id, &exact), MAX_PAGE_BYTES);
    let over_id = serde_json::json!("i".repeat(MAX_PAGE_BYTES - fixed + 1));
    assert!(
        render_fixture(&request, &over_id, 1, rows)
            .unwrap_err()
            .to_string()
            .contains("root_symbols_page_budget_exceeded")
    );
    assert!(render_fixture(&request, &serde_json::json!("\n".repeat(9000)), 0, vec![]).is_err());
    assert!(
        render_fixture(
            &request,
            &serde_json::json!(0),
            1,
            vec![row(0, "x".repeat(MAX_PAGE_BYTES))]
        )
        .is_err()
    );
    assert!(
        render(
            &request,
            &serde_json::json!(0),
            &format!(
                "codestory://symbols/root?project={}",
                "x".repeat(MAX_PAGE_BYTES)
            ),
            "anonymous-project",
            publication(),
            &"a".repeat(64),
            0,
            vec![]
        )
        .is_err()
    );
}

#[test]
fn independent_root_complete_traversal_exceeds_old_inventory_cap() {
    // Serializer-only traversal control; owning runtime fixture must separately
    // prove the full inventory (rather than the former 300/2000 prefix) reaches it.
    let inventory: Vec<_> = (0..2103)
        .map(|i| row(i, format!("anonymous-{i:04}-{}", "\\\"".repeat(30))))
        .collect();
    let mut request = PageRequest {
        limit: 200,
        cursor: None,
    };
    let mut seen = Vec::new();
    let id = serde_json::json!("\n".repeat(512));
    let mut page_count = 0;
    loop {
        let start = offset(&request).unwrap();
        let offered = inventory
            .iter()
            .skip(start)
            .take(request.limit)
            .cloned()
            .collect();
        let response = render_fixture(&request, &id, inventory.len(), offered).unwrap();
        let value = body(&response);
        let rows = value["symbols"].as_array().unwrap();
        assert!(!rows.is_empty());
        assert!(wire_bytes(&id, &response) <= MAX_PAGE_BYTES);
        assert_eq!(value["start_offset"].as_u64().unwrap() as usize, start);
        assert_eq!(
            value["returned_count"].as_u64().unwrap() as usize,
            rows.len()
        );
        assert_eq!(
            value["remaining_count"].as_u64().unwrap() as usize,
            inventory.len() - start - rows.len()
        );
        seen.extend(rows.iter().map(|r| r["id"].as_u64().unwrap() as usize));
        page_count += 1;
        assert!(page_count <= inventory.len());
        if value["continuation_uri"].is_null() {
            assert_eq!(value["truncated"], false);
            break;
        }
        request = next_request(&response);
        // Limits may change without changing the publication/inventory binding.
        request.limit = if page_count % 2 == 0 { 200 } else { 37 };
    }
    assert!(page_count > 11);
    assert_eq!(seen, (0..2103).collect::<Vec<_>>());
}
