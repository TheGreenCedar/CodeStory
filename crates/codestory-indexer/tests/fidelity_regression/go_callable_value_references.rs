//! Callable values are references, not evidence that their targets execute.
use super::*;
use codestory_contracts::graph::{NodeId, OccurrenceKind};
use std::path::Path;

fn refresh(root: &Path, store: &mut Storage, names: &[&str]) -> anyhow::Result<()> {
    let existing_file_ids = store
        .get_files()?
        .into_iter()
        .map(|file| (file.path, file.id))
        .collect();
    WorkspaceIndexer::new(root.to_path_buf()).run_incremental(
        store,
        &codestory_workspace::RefreshInfo {
            mode: codestory_workspace::BuildMode::Incremental,
            files_to_index: names.iter().map(|name| root.join(name)).collect(),
            files_to_remove: vec![],
            existing_file_ids,
        },
        &EventBus::new(),
        None,
    )?;
    anyhow::ensure!(
        store.get_errors(None)?.is_empty(),
        "fixture must index without errors"
    );
    Ok(())
}

fn line(source: &str, anchor: &str) -> u32 {
    let byte = source.find(anchor).expect("unique fixture anchor");
    source[..byte].bytes().filter(|byte| *byte == b'\n').count() as u32 + 1
}

fn declared(store: &Storage, source: &str, anchor: &str) -> Node {
    let expected = line(source, anchor);
    let name = anchor
        .rsplit(' ')
        .next()
        .unwrap()
        .split('(')
        .next()
        .unwrap();
    let found: Vec<_> = store
        .get_nodes()
        .unwrap()
        .into_iter()
        .filter(|node| {
            matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                && node.start_line == Some(expected)
                && node.serialized_name.rsplit('.').next() == Some(name)
        })
        .collect();
    assert_eq!(found.len(), 1, "declaration {anchor}: {found:?}");
    found.into_iter().next().unwrap()
}

fn value_reference(
    store: &Storage,
    source: &str,
    anchor: &str,
    token: &str,
    target: NodeId,
    owner: NodeId,
) {
    value_reference_at(store, source, anchor, token, target, owner, 0);
}

fn value_reference_at(
    store: &Storage,
    source: &str,
    anchor: &str,
    token: &str,
    target: NodeId,
    owner: NodeId,
    ordinal: usize,
) {
    let edges = store.get_edges().unwrap();
    let row = line(source, anchor);
    let source_line = source.lines().nth(row as usize - 1).unwrap();
    let column = source_line
        .match_indices(token)
        .nth(ordinal)
        .expect("token in observed source row")
        .0 as u32
        + 1;
    let usages: Vec<_> = edges
        .iter()
        .filter(|edge| {
            edge.kind == EdgeKind::USAGE
                && edge.line == Some(row)
                && edge.effective_target() == target
        })
        .collect();
    assert!(
        !usages.is_empty(),
        "missing callable-value USAGE at {row}:{column}: {edges:#?}"
    );
    if source_line.matches(token).count() == 1 {
        assert_eq!(
            usages.len(),
            1,
            "one value site must not duplicate its relationship"
        );
    }
    assert!(
        usages.iter().all(|usage| usage.effective_source() == owner),
        "all references on the row must belong to the exact named carrier"
    );
    let usage = usages[0];
    assert_eq!(
        usage.effective_source(),
        owner,
        "value use must retain the exact expected named carrier"
    );
    let mut visible_references = store.get_occurrences_for_element(target.0).unwrap();
    for edge in &usages {
        visible_references.extend(store.get_occurrences_for_element(edge.id.0).unwrap());
    }
    assert!(
        visible_references.iter().any(|occurrence| {
            occurrence.kind == OccurrenceKind::REFERENCE
                && (occurrence.element_id == target.0
                    || usages.iter().any(|edge| edge.id.0 == occurrence.element_id))
                && occurrence.location.file_node_id == usage.file_node_id.unwrap()
                && occurrence.location.start_line == row
                && occurrence.location.end_line == row
                && occurrence.location.start_col == column
                && occurrence.location.end_col == column + token.len() as u32
        }),
        "callable value must retain an exact identifier REFERENCE at {row}:{column}"
    );
    assert!(
        !edges.iter().any(|edge| edge.kind == EdgeKind::CALL
            && edge.line == Some(row)
            && edge.effective_target() == target),
        "a function value does not call its target at row {row}"
    );
}

#[test]
fn go_callable_values_have_exact_references_without_changing_calls() -> anyhow::Result<()> {
    let source = concat!(
        "package values\n",
        "type Worker struct { Field int }\n",
        "func Run() {}\n",
        "func Ready() {}\n",
        "func accept(f func()) {}\n",
        "func pair(a, b func()) {}\n",
        "func (s *Worker) Run() {}\n",
        "func (s *Worker) Ready() {}\n",
        "func (s *Worker) bind() func() {\n",
        "  accept(s.Run)\n",
        "  accept(s.Ready)\n",
        "  a := s.Run\n",
        "  _ = a\n",
        "  pair(s.Run, s.Ready)\n",
        "  /* λ */ pair(s.Run, s.Run)\n",
        "  s.Run()\n",
        "  accept(Run)\n",
        "  b := Ready\n",
        "  _ = b\n",
        "  return s.Ready\n",
        "}\n",
        "func returnBare() func() { return Run }\n",
        "func parameter(w *Worker) func() { return w.Run }\n",
    );
    let dir = tempdir()?;
    fs::write(dir.path().join("values.go"), source)?;
    let mut store = Storage::new_in_memory()?;
    refresh(dir.path(), &mut store, &["values.go"])?;
    let method = declared(&store, source, "func (s *Worker) Run()");
    let ready = declared(&store, source, "func (s *Worker) Ready()");
    let bare = declared(&store, source, "func Run()");
    let bare_ready = declared(&store, source, "func Ready()");
    let bind = declared(&store, source, "func (s *Worker) bind()");
    let return_bare = declared(&store, source, "func returnBare()");
    let parameter = declared(&store, source, "func parameter(w");
    for (anchor, token, target) in [
        ("  accept(s.Run)", "Run", method.id),
        ("  accept(s.Ready)", "Ready", ready.id),
        ("  a := s.Run", "Run", method.id),
        ("  pair(s.Run, s.Ready)", "Run", method.id),
        ("  pair(s.Run, s.Ready)", "Ready", ready.id),
        ("  accept(Run)", "Run", bare.id),
        ("  b := Ready", "Ready", bare_ready.id),
        ("  return s.Ready", "Ready", ready.id),
        ("func returnBare()", "Run", bare.id),
        ("func parameter(", "Run", method.id),
    ] {
        let owner = if anchor.starts_with("func returnBare") {
            return_bare.id
        } else if anchor.starts_with("func parameter") {
            parameter.id
        } else {
            bind.id
        };
        value_reference(&store, source, anchor, token, target, owner);
    }
    for ordinal in [0, 1] {
        value_reference_at(
            &store,
            source,
            "  /* λ */ pair(s.Run, s.Run)",
            "Run",
            method.id,
            bind.id,
            ordinal,
        );
    }
    let nodes = store.get_nodes()?;
    let edges = store.get_edges()?;
    let calls: Vec<_> = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .collect();
    let mut actual: Vec<_> = calls
        .iter()
        .map(|edge| {
            let target = nodes
                .iter()
                .find(|node| node.id == edge.effective_target())
                .unwrap();
            (
                edge.line.unwrap(),
                nodes
                    .iter()
                    .find(|node| node.id == edge.effective_source())
                    .unwrap()
                    .serialized_name
                    .rsplit('.')
                    .next()
                    .unwrap()
                    .to_string(),
                target
                    .serialized_name
                    .rsplit('.')
                    .next()
                    .unwrap()
                    .to_string(),
            )
        })
        .collect();
    actual.sort();
    assert_eq!(
        actual,
        vec![
            (10, "bind".into(), "accept".into()),
            (11, "bind".into(), "accept".into()),
            (14, "bind".into(), "pair".into()),
            (15, "bind".into(), "pair".into()),
            (16, "bind".into(), "Run".into()),
            (17, "bind".into(), "accept".into())
        ],
        "all and only the six fixture invocation sites survive, including the receiver call"
    );
    assert!(
        calls
            .iter()
            .filter(|edge| edge.line == Some(16))
            .all(|edge| edge.effective_target() == method.id)
    );
    assert!(
        !edges.iter().any(|edge| edge.kind == EdgeKind::USAGE
            && edge.line == Some(16)
            && edge.effective_target() == method.id),
        "direct invocation must not also become a value USAGE"
    );
    Ok(())
}

#[test]
fn go_callable_values_respect_binding_and_syntax_boundaries() -> anyhow::Result<()> {
    let source = concat!(
        "package values\n",
        "type Worker struct { Field int }\n",
        "type Other struct {}\n",
        "type Dynamic interface { Run() }\n",
        "func Run() {}\n",
        "func (s *Worker) Run() {}\n",
        "func (s *Other) Run() {}\n",
        "func accept(f func()) {}\n",
        "func valid(s *Worker) { accept(s.Run) }\n",
        "func shadows(Run func(), s *Worker, d Dynamic, unknown interface{}) {\n",
        "  accept(Run)\n",
        "  _ = s.Field\n",
        "  _ = d.Run\n",
        "  _ = unknown.Run\n",
        "  alias := Run\n",
        "  accept(alias)\n",
        "  { s := Other{}; _ = s.Run }\n",
        "  _ = s.Run\n",
        "}\n",
        "func localShadow() { Run := func() {}; accept(Run) }\n",
        "func opaque(s *Worker) { _ = \"s.Run\"; /* s.Run */ _ = s.Field }\n",
        "func closure(s *Worker) { _ = func() { accept(s.Run) } }\n",
    );
    let dir = tempdir()?;
    fs::write(dir.path().join("bindings.go"), source)?;
    let mut store = Storage::new_in_memory()?;
    refresh(dir.path(), &mut store, &["bindings.go"])?;
    let worker = declared(&store, source, "func (s *Worker) Run()");
    let other = declared(&store, source, "func (s *Other) Run()");
    value_reference(
        &store,
        source,
        "func valid(",
        "Run",
        worker.id,
        declared(&store, source, "func valid(s").id,
    );
    value_reference(
        &store,
        source,
        "  { s := Other{}",
        "Run",
        other.id,
        declared(&store, source, "func shadows(Run").id,
    );
    value_reference(
        &store,
        source,
        "  _ = s.Run",
        "Run",
        worker.id,
        declared(&store, source, "func shadows(Run").id,
    );
    value_reference(
        &store,
        source,
        "func closure(",
        "Run",
        worker.id,
        declared(&store, source, "func closure(s").id,
    );
    let edges = store.get_edges()?;
    let callable_ids: HashSet<_> = store
        .get_nodes()?
        .iter()
        .filter(|node| matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD))
        .map(|node| node.id)
        .collect();
    for anchor in [
        "  accept(Run)",
        "  _ = s.Field",
        "  _ = d.Run",
        "  _ = unknown.Run",
        "  alias := Run",
        "  accept(alias)",
        "func localShadow()",
        "func opaque(",
    ] {
        let row = line(source, anchor);
        assert!(
            !edges.iter().any(|edge| edge.kind == EdgeKind::USAGE
                && edge.line == Some(row)
                && callable_ids.contains(&edge.effective_target())),
            "unknown/noncallable/shadowed syntax must not be guessed as a callable at {row}: {edges:#?}"
        );
    }
    let refs = store.get_occurrences()?;
    for row in [5, 6, 7] {
        let column = source.lines().nth(row - 1).unwrap().rfind("Run").unwrap() as u32 + 1;
        assert!(
            !refs.iter().any(|occ| occ.kind == OccurrenceKind::REFERENCE
                && occ.location.start_line == row as u32
                && occ.location.start_col == column),
            "a callable definition name is not a value use"
        );
    }
    // Imported spelling and shadowing are not proof of an authenticated target.
    for (imports, body) in [
        ("import ext \"example.com/missing\"\n", "_ = ext.Run"),
        (
            "import ext \"example.com/missing\"\n",
            "ext := struct { Run func() }{}; _ = ext.Run",
        ),
        ("import . \"example.com/missing\"\n", "_ = Unknown"),
        ("import _ \"example.com/missing\"\n", "_ = Unknown"),
    ] {
        let fixture = format!(
            "package values\n{imports}func Run() {{}}\nfunc Missing() {{}}\nfunc guard() {{ {body} }}\n"
        );
        let project = tempdir()?;
        fs::write(project.path().join("imports.go"), &fixture)?;
        let mut imported = Storage::new_in_memory()?;
        refresh(project.path(), &mut imported, &["imports.go"])?;
        let guard_row = line(&fixture, "func guard()");
        let declarations: HashSet<_> = imported
            .get_nodes()?
            .into_iter()
            .filter(|node| {
                matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                    && node.start_line != Some(guard_row)
            })
            .map(|node| node.id)
            .collect();
        assert!(
            !imported
                .get_edges()?
                .iter()
                .any(|edge| edge.kind == EdgeKind::USAGE
                    && edge.line == Some(guard_row)
                    && declarations.contains(&edge.effective_target())),
            "unproven imported/ambiguous value must not gain a bound usage: {fixture}"
        );
    }
    let ambiguous = tempdir()?;
    for (name, text) in [
        ("left.go", "package values\nfunc shared() {}\n"),
        ("right.go", "package values\nfunc shared() {}\n"),
        ("use.go", "package values\nfunc guard() { _ = shared }\n"),
    ] {
        fs::write(ambiguous.path().join(name), text)?;
    }
    let mut duplicates = Storage::new_in_memory()?;
    refresh(
        ambiguous.path(),
        &mut duplicates,
        &["left.go", "right.go", "use.go"],
    )?;
    let duplicate_targets: HashSet<_> = duplicates
        .get_nodes()?
        .into_iter()
        .filter(|node| {
            node.kind == NodeKind::FUNCTION
                && node.serialized_name.rsplit('.').next() == Some("shared")
        })
        .map(|node| node.id)
        .collect();
    assert!(
        !duplicates
            .get_edges()?
            .iter()
            .any(|edge| edge.kind == EdgeKind::USAGE
                && duplicate_targets.contains(&edge.effective_target())),
        "ambiguous package declarations must not produce a guessed bound callable reference"
    );
    Ok(())
}

#[test]
fn go_cross_file_values_persist_and_removed_sites_disappear() -> anyhow::Result<()> {
    let owner =
        "package values\ntype Worker struct {}\nfunc (s *Worker) Run() {}\nfunc helper() {}\n";
    let client = "package values\nfunc consume(f func()) {}\nfunc bind(w *Worker) func() {\n  consume(w.Run)\n  consume(helper)\n  return w.Run\n}\n";
    let dir = tempdir()?;
    fs::write(
        dir.path().join("go.mod"),
        "module example.com/values\n\ngo 1.24\n",
    )?;
    fs::write(dir.path().join("owner.go"), owner)?;
    fs::write(dir.path().join("client.go"), client)?;
    let mut store = Storage::new_in_memory()?;
    refresh(dir.path(), &mut store, &["go.mod", "owner.go", "client.go"])?;
    let target = declared(&store, owner, "func (s *Worker) Run()");
    let helper = declared(&store, owner, "func helper()");
    let bind = declared(&store, client, "func bind(w");
    for _ in 0..2 {
        value_reference(
            &store,
            client,
            "  consume(w.Run)",
            "Run",
            target.id,
            bind.id,
        );
        value_reference(
            &store,
            client,
            "  consume(helper)",
            "helper",
            helper.id,
            bind.id,
        );
        value_reference(&store, client, "  return w.Run", "Run", target.id, bind.id);
        refresh(dir.path(), &mut store, &["owner.go", "client.go"])?;
    }
    fs::write(
        dir.path().join("client.go"),
        "package values\nfunc bind(w *Worker) {}\n",
    )?;
    refresh(dir.path(), &mut store, &["client.go"])?;
    assert!(
        !store
            .get_edges()?
            .iter()
            .any(|edge| edge.kind == EdgeKind::USAGE
                && [target.id, helper.id].contains(&edge.effective_target())),
        "incremental removal must not retain stale value references"
    );
    Ok(())
}

#[test]
fn go_callable_value_type_shadows_never_bind_package_methods() -> anyhow::Result<()> {
    let local_type = concat!(
        "package values\n",
        "type Worker struct{}\n",
        "func (*Worker) Run() {}\n",
        "func control(w *Worker) { _ = w.Run }\n",
        "func f() {\n",
        "  type Worker struct { Run func() }\n",
        "  var x Worker\n",
        "  _ = x.Run\n",
        "}\n",
        "func origin(w *Worker) {\n",
        "  {\n",
        "    type Worker struct { Run func() }\n",
        "    _ = w.Run\n",
        "  }\n",
        "}\n",
        "type Other struct{}\n",
        "func (*Other) Run() {}\n",
        "func signature_type_origin(w *Worker) {\n",
        "  type Callback func(w *Other)\n",
        "  _ = w.Run // callback-signature control\n",
        "}\n",
        "func helper() {}\n",
        "func bare_visibility_origin() {\n",
        "  type Callback func(helper int)\n",
        "  _ = helper\n",
        "}\n",
    );
    let type_parameter = concat!(
        "package values\n",
        "type T struct{}\n",
        "func (*T) Run() {}\n",
        "func control(t *T) { _ = t.Run }\n",
        "func f[T interface{ Run() }](t T) {\n",
        "  _ = t.Run\n",
        "}\n",
        "func origin(w *T) {\n",
        "  type Box[T any] struct{ Value T }\n",
        "  _ = w.Run\n",
        "}\n",
    );
    let mut failures = Vec::new();
    for (name, source, method_anchor, canonical_name, control_anchor, bad_anchor) in [
        (
            "local_type",
            local_type,
            "func (*Worker) Run()",
            "Worker.Run",
            "func control(w",
            "  _ = x.Run",
        ),
        (
            "type_parameter",
            type_parameter,
            "func (*T) Run()",
            "T.Run",
            "func control(t",
            "  _ = t.Run",
        ),
    ] {
        let project = tempdir()?;
        fs::write(project.path().join("shadow.go"), source)?;
        let mut store = Storage::new_in_memory()?;
        refresh(project.path(), &mut store, &["shadow.go"])?;
        let files = store.get_files()?;
        assert_eq!(
            files.len(),
            1,
            "{name}: the supplied Go file must be indexed"
        );
        assert!(
            files
                .iter()
                .all(|file| file.indexed && file.complete && file.language == "go"),
            "{name}: a parse failure or unindexed fixture cannot satisfy the negative control: {files:?}"
        );
        let method = declared(&store, source, method_anchor);
        assert_eq!(
            method.kind,
            NodeKind::METHOD,
            "{name}: package method must exist"
        );
        assert_eq!(method.serialized_name, canonical_name);
        assert!(
            method.canonical_id.is_some(),
            "{name}: package method must have canonical identity"
        );
        let control = declared(&store, source, control_anchor);
        value_reference(&store, source, control_anchor, "Run", method.id, control.id);
        if name == "local_type" {
            let origin = declared(&store, source, "func origin(w");
            value_reference(&store, source, "    _ = w.Run", "Run", method.id, origin.id);
            let other = declared(&store, source, "func (*Other) Run()");
            assert_eq!(other.serialized_name, "Other.Run");
            assert_eq!(other.kind, NodeKind::METHOD);
            let signature_origin = declared(&store, source, "func signature_type_origin(w");
            value_reference(
                &store,
                source,
                "  _ = w.Run // callback-signature control",
                "Run",
                method.id,
                signature_origin.id,
            );
            let helper = declared(&store, source, "func helper()");
            assert_eq!(helper.kind, NodeKind::FUNCTION);
            assert!(helper.canonical_id.is_some());
            let bare_origin = declared(&store, source, "func bare_visibility_origin()");
            value_reference(
                &store,
                source,
                "  _ = helper",
                "helper",
                helper.id,
                bare_origin.id,
            );
        } else if name == "type_parameter" {
            let origin = declared(&store, source, "func origin(w");
            value_reference(&store, source, "  _ = w.Run", "Run", method.id, origin.id);
        }
        let bad_row = line(source, bad_anchor);
        let edges = store.get_edges()?;
        if edges.iter().any(|edge| {
            edge.kind == EdgeKind::USAGE
                && edge.line == Some(bad_row)
                && edge.effective_target() == method.id
        }) {
            failures.push(format!("{name}: shadowed type spelling falsely binds a field/interface value to {canonical_name} at {bad_row}: {edges:#?}"));
        }
        if store
            .get_occurrences_for_element(method.id.0)?
            .iter()
            .any(|occurrence| {
                occurrence.kind == OccurrenceKind::REFERENCE
                    && occurrence.location.start_line == bad_row
            })
        {
            failures.push(format!(
                "{name}: shadowed value acquired canonical package-method reference occurrence"
            ));
        }
        assert!(
            !edges.iter().any(|edge| edge.kind == EdgeKind::CALL),
            "{name}: neither value expression invokes its target"
        );
    }
    assert!(
        failures.is_empty(),
        "type-shadow binding failures: {}",
        failures.join("\n")
    );
    Ok(())
}
