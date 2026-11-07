//! Callable values are references, not evidence that their targets execute.
use super::*;
use codestory_contracts::graph::{NodeId, OccurrenceKind};
use std::path::Path;

fn refresh(
    root: &Path,
    store: &mut Storage,
    names: &[&str],
) -> anyhow::Result<codestory_indexer::IncrementalIndexingStats> {
    let existing_file_ids = store
        .get_files()?
        .into_iter()
        .map(|file| (file.path, file.id))
        .collect();
    let stats = WorkspaceIndexer::new(root.to_path_buf()).run_incremental(
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
    Ok(stats)
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
fn go_package_variables_keep_identifier_definitions_and_full_initializers() -> anyhow::Result<()> {
    let source = r#"package values
type action func()
func wrap(fn action) action { return fn }
func deferred() {}
func pair() (int, int) { return 1, 2 }
func before() { Late() }
var Late = wrap(func() { deferred() })
var Plain int
var Wrapped = wrap(func() {
    deferred()
})
var Left, Right = pair()
var (
    Grouped = wrap(func() { deferred() })
    GroupedLeft, GroupedRight = pair()
    Declared action
)
// public is ordinary comment text, not Go visibility.
var privateBinding = "public"
// private is ordinary comment text, not Go visibility.
var Exported = "private"
var _ = wrap(deferred)
var _, Visible = pair()
const Constant = 1
func consume() {
    Wrapped := func() {}
    Wrapped()
    var Local = wrap(deferred)
    _ = Local
}
func invoke() { Wrapped() }
"#;
    let dir = tempdir()?;
    fs::write(dir.path().join("variables.go"), source)?;
    let mut store = Storage::new_in_memory()?;
    refresh(dir.path(), &mut store, &["variables.go"])?;
    let nodes = store.get_nodes()?;
    let globals: Vec<_> = nodes
        .iter()
        .filter(|node| node.kind == NodeKind::GLOBAL_VARIABLE)
        .collect();
    let expected = [
        ("Late", "var Late = wrap(func() { deferred() })"),
        ("Plain", "var Plain int"),
        ("Wrapped", "var Wrapped = wrap(func() {\n    deferred()\n})"),
        ("Left", "var Left, Right = pair()"),
        ("Right", "var Left, Right = pair()"),
        ("Grouped", "Grouped = wrap(func() { deferred() })"),
        ("GroupedLeft", "GroupedLeft, GroupedRight = pair()"),
        ("GroupedRight", "GroupedLeft, GroupedRight = pair()"),
        ("Declared", "Declared action"),
        ("privateBinding", "var privateBinding = \"public\""),
        ("Exported", "var Exported = \"private\""),
        ("Visible", "var _, Visible = pair()"),
    ];
    let mut actual_names: Vec<_> = globals
        .iter()
        .map(|node| node.serialized_name.as_str())
        .collect();
    actual_names.sort();
    let mut expected_names: Vec<_> = expected.iter().map(|(name, _)| *name).collect();
    expected_names.sort();
    assert_eq!(
        actual_names, expected_names,
        "only package variables are declarations"
    );
    let projections = store.get_canonical_search_symbol_detail_batch_after(None, 128)?;
    for (name, declaration) in expected {
        let variable = globals
            .iter()
            .find(|node| node.serialized_name == name)
            .unwrap();
        let start = source.find(declaration).unwrap();
        let name_start = start + declaration.find(name).unwrap();
        let name_line = source[..name_start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count() as u32
            + 1;
        let name_col = source[..name_start].rsplit('\n').next().unwrap().len() as u32 + 1;
        let definitions: Vec<_> = store
            .get_occurrences_for_element(variable.id.0)?
            .into_iter()
            .filter(|occurrence| occurrence.kind == OccurrenceKind::DEFINITION)
            .collect();
        assert_eq!(definitions.len(), 1, "one identifier definition for {name}");
        let location = &definitions[0].location;
        assert_eq!(
            (
                location.start_line,
                location.start_col,
                location.end_line,
                location.end_col
            ),
            (name_line, name_col, name_line, name_col + name.len() as u32),
            "{name} occurrence must remain the identifier, not its initializer"
        );
        let line_offsets: Vec<_> = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(index, _)| index + 1))
            .collect();
        let display_start = line_offsets[variable.start_line.unwrap() as usize - 1]
            + variable.start_col.unwrap() as usize
            - 1;
        let display_end = line_offsets[variable.end_line.unwrap() as usize - 1]
            + variable.end_col.unwrap() as usize
            - 1;
        assert_eq!(
            &source[display_start..display_end],
            declaration,
            "complete declaration for {name}"
        );
        assert!(
            projections.iter().any(|row| row.node_id == variable.id
                && row.display_name == name
                && row.start_line == variable.start_line
                && row.end_line == variable.end_line),
            "{name} must reach canonical source search with its full span"
        );
        assert_eq!(
            store.get_component_access(variable.id)?,
            Some(if name == "privateBinding" {
                codestory_contracts::graph::AccessKind::Private
            } else {
                codestory_contracts::graph::AccessKind::Public
            }),
            "Go visibility follows the variable name, not nearby text"
        );
    }
    let file = nodes
        .iter()
        .find(|node| node.kind == NodeKind::FILE)
        .unwrap();
    let edges = store.get_edges()?;
    let eager: Vec<_> = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.effective_source() == file.id)
        .collect();
    assert_eq!(
        eager.len(),
        7,
        "all seven package initializer calls, including discarded blank values, execute eagerly"
    );
    assert!(
        !edges.iter().any(|edge| edge.kind == EdgeKind::CALL
            && globals.iter().any(
                |node| node.id == edge.effective_source() || node.id == edge.effective_target()
            )),
        "variable discovery cannot assert callable identity or ownership"
    );
    for anchor in [
        "    deferred()",
        "    Grouped = wrap(func() { deferred() })",
    ] {
        assert!(
            !edges.iter().any(|edge| edge.kind == EdgeKind::CALL
                && edge.line == Some(line(source, anchor))
                && nodes
                    .iter()
                    .any(|node| node.id == edge.effective_target()
                        && node.serialized_name == "deferred")),
            "deferred literal body cannot acquire a CALL"
        );
    }
    let revision = codestory_indexer::current_file_extraction_revision("go").unwrap();
    assert!(store.file_extraction_identity_matches(file.id.0, "go", revision)?);
    assert!(
        !store.file_extraction_identity_matches(
            file.id.0,
            "go",
            "canonical-reference-occurrences-v1;go-callable-value-references-v4"
        )?,
        "the old producer revision must not admit a newly indexed variable publication"
    );
    let identities = |store: &Storage| -> anyhow::Result<_> {
        let mut nodes = store.get_nodes()?;
        nodes.sort_by_key(|node| node.id);
        let mut occurrences = store.get_occurrences()?;
        occurrences.sort_by_key(|occurrence| {
            let location = &occurrence.location;
            (
                occurrence.element_id,
                occurrence.kind as i32,
                location.file_node_id,
                location.start_line,
                location.start_col,
                location.end_line,
                location.end_col,
            )
        });
        Ok((
            serde_json::to_value(nodes)?,
            serde_json::to_value(occurrences)?,
        ))
    };
    let fresh = identities(&store)?;
    let warm = refresh(dir.path(), &mut store, &["variables.go"])?;
    assert_eq!(
        warm.artifact_cache_hits, 1,
        "identity comparison requires an actual cached artifact replay"
    );
    assert_eq!(
        identities(&store)?,
        fresh,
        "a call before its package binding cannot reorder cached declaration/reference identities"
    );
    Ok(())
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

#[test]
fn go_ordinary_field_references_bind_paths_and_refresh_unchanged_readers() -> anyhow::Result<()> {
    use codestory_contracts::graph::{TrailCallerScope, TrailConfig, TrailDirection, TrailMode};
    let types = "package fields\ntype Config struct { DataPath string }\ntype Other struct { DataPath string }\ntype Server struct { config *Config; alternate *Other }\n";
    let reads = "package fields\nfunc consume(v string) {}\nfunc direct(c *Config) { _ = c.DataPath }\nfunc (s *Server) nested() {\n _ = s.config.DataPath\n _ = s.alternate.DataPath\n consume(s.config.DataPath)\n}\n";
    let dir = tempdir()?;
    fs::write(
        dir.path().join("go.mod"),
        "module example.com/fields\n\ngo 1.24\n",
    )?;
    fs::write(dir.path().join("types.go"), types)?;
    fs::write(dir.path().join("reads.go"), reads)?;
    let mut store = Storage::new_in_memory()?;
    refresh(dir.path(), &mut store, &["go.mod", "types.go", "reads.go"])?;
    let field = |store: &Storage, name: &str| {
        let found: Vec<_> = store
            .get_nodes()
            .unwrap()
            .into_iter()
            .filter(|node| node.kind == NodeKind::FIELD && node.serialized_name == name)
            .collect();
        assert_eq!(found.len(), 1, "unique field {name}: {found:?}");
        found[0].id
    };
    let config = field(&store, "Config.DataPath");
    let other = field(&store, "Other.DataPath");
    let direct = declared(&store, reads, "func direct(c").id;
    let nested = declared(&store, reads, "func (s *Server) nested()").id;
    for (anchor, target, carrier) in [
        ("func direct(c", config, direct),
        (" _ = s.config.DataPath", config, nested),
        (" _ = s.alternate.DataPath", other, nested),
        (" consume(s.config.DataPath)", config, nested),
    ] {
        value_reference(&store, reads, anchor, "DataPath", target, carrier);
    }
    let calls = |store: &Storage| {
        let mut rows: Vec<_> = store
            .get_edges()
            .unwrap()
            .into_iter()
            .filter(|edge| edge.kind == EdgeKind::CALL)
            .map(|edge| format!("{edge:?}"))
            .collect();
        rows.sort();
        rows
    };
    let original_calls = calls(&store);
    assert_eq!(original_calls.len(), 1, "only explicit consume invocation");
    let incoming = store.get_trail(&TrailConfig {
        root_id: config,
        mode: TrailMode::AllReferencing,
        direction: TrailDirection::Incoming,
        depth: 1,
        max_nodes: 30,
        caller_scope: TrailCallerScope::IncludeTestsAndBenches,
        edge_filter: vec![EdgeKind::USAGE],
        ..Default::default()
    })?;
    assert_eq!(
        incoming.edges.len(),
        3,
        "all Config reads visible through the existing incoming reader"
    );
    assert!(incoming.nodes.iter().any(|node| node.id == direct));
    assert!(incoming.nodes.iter().any(|node| node.id == nested));
    for edge in incoming.edges {
        let refs = store.get_occurrences_for_element(edge.id.0)?;
        assert!(
            refs.iter().any(|occ| occ.kind == OccurrenceKind::REFERENCE
                && occ.location.start_line == edge.line.unwrap()
                && occ.location.end_col - occ.location.start_col == 8),
            "edge occurrence consumer"
        );
    }
    let warm = refresh(dir.path(), &mut store, &["go.mod", "types.go", "reads.go"])?;
    assert_eq!(
        warm.artifact_cache_hits, 2,
        "both Go files must replay cached artifacts"
    );
    value_reference(&store, reads, "func direct(c", "DataPath", config, direct);
    value_reference(
        &store,
        reads,
        " _ = s.config.DataPath",
        "DataPath",
        config,
        nested,
    );
    value_reference(
        &store,
        reads,
        " _ = s.alternate.DataPath",
        "DataPath",
        other,
        nested,
    );
    assert_eq!(
        calls(&store),
        original_calls,
        "unchanged warm refresh preserves the CALL multiset"
    );
    fs::write(
        dir.path().join("types.go"),
        types.replace("config *Config", "config *Other"),
    )?;
    refresh(dir.path(), &mut store, &["types.go"])?;
    for anchor in [" _ = s.config.DataPath", " consume(s.config.DataPath)"] {
        value_reference(&store, reads, anchor, "DataPath", other, nested);
        assert!(
            !store
                .get_edges()?
                .iter()
                .any(|edge| edge.kind == EdgeKind::USAGE
                    && edge.line == Some(line(reads, anchor))
                    && edge.effective_target() == config),
            "unchanged reader must lose stale field binding"
        );
    }
    value_reference(&store, reads, "func direct(c", "DataPath", config, direct);
    assert_eq!(
        calls(&store),
        original_calls,
        "field metadata edits cannot alter calls"
    );
    let removed = store
        .get_files()?
        .into_iter()
        .find(|file| file.path.ends_with("types.go"))
        .unwrap()
        .id;
    let file_ids = store
        .get_files()?
        .into_iter()
        .map(|file| (file.path, file.id))
        .collect();
    fs::remove_file(dir.path().join("types.go"))?;
    WorkspaceIndexer::new(dir.path().to_path_buf()).run_incremental(
        &mut store,
        &codestory_workspace::RefreshInfo {
            mode: codestory_workspace::BuildMode::Incremental,
            files_to_index: vec![],
            files_to_remove: vec![removed],
            existing_file_ids: file_ids,
        },
        &EventBus::new(),
        None,
    )?;
    assert!(
        !store
            .get_edges()?
            .iter()
            .any(|edge| edge.kind == EdgeKind::USAGE && edge.resolved_target.is_some()),
        "removal-only refresh clears field bindings"
    );
    fs::write(dir.path().join("types.go"), types)?;
    refresh(dir.path(), &mut store, &["types.go"])?;
    value_reference(&store, reads, "func direct(c", "DataPath", config, direct);
    value_reference(
        &store,
        reads,
        " _ = s.config.DataPath",
        "DataPath",
        config,
        nested,
    );
    value_reference(
        &store,
        reads,
        " _ = s.alternate.DataPath",
        "DataPath",
        other,
        nested,
    );
    assert_eq!(
        calls(&store),
        original_calls,
        "restored declarations rebind unchanged readers without changing calls"
    );
    Ok(())
}

#[test]
fn go_ordinary_field_references_reject_unsupported_and_shadowed_owners() -> anyhow::Result<()> {
    let types = "package fields\ntype Config struct { DataPath string }\ntype Alias = Config\ntype Generic[T any] struct { DataPath string }\ntype View interface { Read() }\ntype Nested struct { config struct { DataPath string } }\ntype Embedded struct { Config }\ntype Pair struct { left, right *Config }\n";
    let reads = "package fields\nimport ext \"example.com/missing\"\nfunc alias(c *Alias) { _ = c.DataPath }\nfunc generic(c *Generic[int]) { _ = c.DataPath }\nfunc opaque(c View) { _ = c.DataPath }\nfunc anonymous(c *Nested) { _ = c.config.DataPath }\nfunc embedded(c *Embedded) { _ = c.DataPath }\nfunc grouped(c *Pair) { _ = c.left.DataPath }\nfunc imported(c *ext.Config) { _ = c.DataPath }\nfunc local() { type Config struct { DataPath int }; c := Config{}; _ = c.DataPath }\nfunc shadow(c *Config) { c := unknown; _ = c.DataPath }\nfunc parameterOrigin(c *Config) { type Config struct { DataPath int }; _ = c.DataPath }\n";
    let dir = tempdir()?;
    fs::write(
        dir.path().join("go.mod"),
        "module example.com/fields\n\ngo 1.24\n",
    )?;
    fs::write(dir.path().join("types.go"), types)?;
    fs::write(dir.path().join("reads.go"), reads)?;
    let mut store = Storage::new_in_memory()?;
    refresh(dir.path(), &mut store, &["go.mod", "types.go", "reads.go"])?;
    let fields: HashSet<_> = store
        .get_nodes()?
        .into_iter()
        .filter(|node| node.kind == NodeKind::FIELD)
        .map(|node| node.id)
        .collect();
    for edge in store.get_edges()? {
        if edge.kind == EdgeKind::USAGE && fields.contains(&edge.effective_target()) {
            assert_eq!(
                edge.line,
                Some(line(reads, "func parameterOrigin")),
                "unsupported field owner must not bind by spelling: {edge:?}"
            );
        }
    }
    let config = store
        .get_nodes()?
        .into_iter()
        .find(|node| {
            node.kind == NodeKind::FIELD
                && node.serialized_name == "Config.DataPath"
                && node.file_node_id
                    == store
                        .get_files()
                        .unwrap()
                        .iter()
                        .find(|f| f.path.ends_with("types.go"))
                        .map(|f| NodeId(f.id))
        })
        .unwrap()
        .id;
    value_reference_at(
        &store,
        reads,
        "func parameterOrigin",
        "DataPath",
        config,
        declared(&store, reads, "func parameterOrigin(c").id,
        1,
    );
    assert!(
        !store
            .get_edges()?
            .iter()
            .any(|edge| edge.kind == EdgeKind::CALL),
        "field reads do not invoke targets"
    );
    Ok(())
}

#[test]
fn go_ordinary_field_references_fail_closed_on_ambiguous_or_unverified_catalogues()
-> anyhow::Result<()> {
    let types = "package fields\ntype Config struct { DataPath string }\n";
    let reads = "package fields\nfunc read(c *Config) { _ = c.DataPath }\n";
    for (name, duplicate) in [
        (
            "duplicate.go",
            "package fields\ntype Config struct { DataPath int }\n",
        ),
        ("alias.go", "package fields\ntype Config = int\n"),
        (
            "generic.go",
            "package fields\ntype Config[T any] struct { DataPath T }\n",
        ),
        (
            "interface.go",
            "package fields\ntype Config interface { Read() }\n",
        ),
        (
            "duplicate_test.go",
            "package fields\ntype Config struct { DataPath int }\n",
        ),
    ] {
        let dir = tempdir()?;
        fs::write(
            dir.path().join("go.mod"),
            "module example.com/fields\n\ngo 1.24\n",
        )?;
        fs::write(dir.path().join("types.go"), types)?;
        fs::write(dir.path().join("reads.go"), reads)?;
        fs::write(dir.path().join(name), duplicate)?;
        let mut store = Storage::new_in_memory()?;
        refresh(
            dir.path(),
            &mut store,
            &["go.mod", "types.go", "reads.go", name],
        )?;
        assert!(
            !store
                .get_edges()?
                .iter()
                .any(|edge| edge.kind == EdgeKind::USAGE && edge.resolved_target.is_some()),
            "ambiguous owner {name} must not pick one field"
        );
    }
    let dir = tempdir()?;
    fs::write(
        dir.path().join("go.mod"),
        "module example.com/fields\n\ngo 1.24\n",
    )?;
    fs::write(dir.path().join("types.go"), types)?;
    fs::write(dir.path().join("reads.go"), reads)?;
    let mut store = Storage::new_in_memory()?;
    refresh(dir.path(), &mut store, &["go.mod", "types.go", "reads.go"])?;
    assert!(
        store
            .get_edges()?
            .iter()
            .any(|edge| edge.kind == EdgeKind::USAGE && edge.resolved_target.is_some()),
        "valid control must first bind"
    );
    fs::write(
        dir.path().join("types.go"),
        types.replace("DataPath string", "DataPath int"),
    )?;
    refresh(dir.path(), &mut store, &["reads.go"])?;
    assert!(
        !store
            .get_edges()?
            .iter()
            .any(|edge| edge.kind == EdgeKind::USAGE && edge.resolved_target.is_some()),
        "unindexed declaration drift must clear old field binding"
    );
    Ok(())
}

#[test]
fn go_ordinary_field_references_keep_package_and_test_identity() -> anyhow::Result<()> {
    for duplicate_module in [false, true] {
        let dir = tempdir()?;
        fs::write(
            dir.path().join("go.mod"),
            "module example.com/identity\n\ngo 1.24\n",
        )?;
        let types = "package fields\ntype Config struct { DataPath string }\n";
        let reads = "package fields\nfunc read(c *Config) { _ = c.DataPath }\n";
        let mut names = vec!["go.mod".to_string()];
        for package in ["left", "right"] {
            fs::create_dir(dir.path().join(package))?;
            if duplicate_module {
                fs::write(
                    dir.path().join(package).join("go.mod"),
                    "module example.com/collision\n\ngo 1.24\n",
                )?;
                names.push(format!("{package}/go.mod"));
            }
            fs::write(dir.path().join(package).join("types.go"), types)?;
            fs::write(dir.path().join(package).join("reads.go"), reads)?;
            names.extend([format!("{package}/types.go"), format!("{package}/reads.go")]);
        }
        // A production reader cannot acquire declarations available only to tests.
        fs::write(
            dir.path().join("hidden_test.go"),
            "package fields\ntype Hidden struct { DataPath string }\n",
        )?;
        fs::write(
            dir.path().join("production.go"),
            "package fields\nfunc hidden(h *Hidden) { _ = h.DataPath }\n",
        )?;
        names.extend(["hidden_test.go".to_string(), "production.go".to_string()]);
        let mut store = Storage::new_in_memory()?;
        let names: Vec<_> = names.iter().map(String::as_str).collect();
        refresh(dir.path(), &mut store, &names)?;
        let files = store.get_files()?;
        let nodes = store.get_nodes()?;
        let edges = store.get_edges()?;
        let field_usages: Vec<_> = edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::USAGE && edge.resolved_target.is_some())
            .collect();
        if duplicate_module {
            assert!(
                field_usages.is_empty(),
                "ambiguous package import path cannot choose one directory: {field_usages:?}"
            );
        } else {
            assert_eq!(
                field_usages.len(),
                2,
                "only two authenticated production-package fields may bind"
            );
            for package in ["left", "right"] {
                let type_file = files
                    .iter()
                    .find(|file| file.path.ends_with(format!("{package}/types.go")))
                    .unwrap()
                    .id;
                let read_file = files
                    .iter()
                    .find(|file| file.path.ends_with(format!("{package}/reads.go")))
                    .unwrap()
                    .id;
                let target = nodes
                    .iter()
                    .find(|node| {
                        node.kind == NodeKind::FIELD
                            && node.serialized_name == "Config.DataPath"
                            && node.file_node_id == Some(NodeId(type_file))
                    })
                    .unwrap()
                    .id;
                let usage = field_usages
                    .iter()
                    .find(|edge| edge.file_node_id == Some(NodeId(read_file)))
                    .expect("own-package field reference");
                assert_eq!(
                    usage.effective_target(),
                    target,
                    "same spelling cannot cross package identity"
                );
                let refs = store.get_occurrences_for_element(usage.id.0)?;
                assert!(
                    refs.iter().any(|occ| occ.kind == OccurrenceKind::REFERENCE
                        && occ.location.file_node_id == NodeId(read_file)
                        && occ.location.start_line == 2
                        && occ.location.end_col - occ.location.start_col == 8),
                    "exact own-file edge reference"
                );
            }
        }
        assert!(
            !edges.iter().any(|edge| edge.kind == EdgeKind::CALL),
            "field package/test binding cannot manufacture calls"
        );
    }
    Ok(())
}
