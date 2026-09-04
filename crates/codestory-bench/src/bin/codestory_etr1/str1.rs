use super::*;
use codestory_contracts::graph::{NodeKind, ResolutionCertainty};
use codestory_store::CoreReadSession;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphNode {
    pub id: i64,
    pub fragment_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Witness {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub content_digest: String,
    pub source: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Relation {
    pub id: String,
    pub source: i64,
    pub target: i64,
    pub certainty: String,
    pub occurrence: Witness,
    pub occurrence_fragment_ids: Vec<String>,
    pub raw: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Graph {
    pub repository_id: String,
    pub preparation: FileBinding,
    pub core: FileBinding,
    pub core_pointer: Value,
    pub source_bindings: Vec<FileBinding>,
    pub nodes: Vec<GraphNode>,
    pub relations: Vec<Relation>,
    pub gaps: Vec<Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub seed_fragment_id: String,
    pub anchors: Vec<i64>,
    pub relations: Vec<Relation>,
    pub eligible: Vec<String>,
    pub excluded_before: Vec<String>,
    pub retained_successors: Vec<String>,
    pub boundary_gaps: Vec<Value>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    operation: String,
    preparation: FileBinding,
    method: FileBinding,
    #[serde(default)]
    graph_inputs: Option<FileBinding>,
    #[serde(default)]
    graph_preparations: BTreeMap<String, FileBinding>,
    #[serde(default)]
    graphs: Option<FileBinding>,
    #[serde(default)]
    vectors: Option<FileBinding>,
    #[serde(default)]
    control_run: Option<FileBinding>,
    #[serde(default)]
    state_root: Option<PathBuf>,
    cancel_file: PathBuf,
    output: PathBuf,
}

#[derive(Debug, Deserialize)]
struct GraphInput {
    repository_id: String,
    preparation: FileBinding,
    core: FileBinding,
    pointer: FileBinding,
}
#[derive(Debug, Deserialize)]
struct GraphInputs {
    repositories: Vec<GraphInput>,
}

fn fixed_inputs(job: &Job, preparation: &Etr1PreparationV1) -> Result<Vec<GraphInput>> {
    ensure!(
        job.method.sha256 == "0902f8f8f6771fce5c6addf09bbdef061fd3706be0da050c32baead80fb171fc",
        "unregistered_structural_method"
    );
    read_bound_json::<Value>(&job.method)?;
    if preparation.authority == "synthetic_canary_only" {
        ensure!(
            preparation.repositories.len() == 1
                && preparation.fragments.len() == 32
                && preparation.wordings.len() == 3,
            "invalid_canary_shape"
        );
        return Ok(Vec::new());
    }
    ensure!(
        preparation.authority == "visible_development_frontier_only",
        "invalid_structural_authority"
    );
    ensure!(
        job.preparation.sha256
            == "30b84d4d848f96bd4fe799f2e0f28b9114971da0e47bf98ebe54fe36242199fd",
        "unregistered_preparation"
    );
    let binding = job
        .graph_inputs
        .as_ref()
        .context("graph_input_freeze_missing")?;
    ensure!(
        binding.sha256 == "668c990ee29b25a4bab0cb03e048d70eebd8ffe3dd62317d69b2e58b912a2c9f",
        "unregistered_graph_inputs"
    );
    let inputs: GraphInputs = read_bound_json(binding)?;
    let expected = preparation
        .repositories
        .iter()
        .map(|r| r.repository_id.as_str())
        .collect::<BTreeSet<_>>();
    ensure!(
        inputs.repositories.len() == expected.len()
            && inputs
                .repositories
                .iter()
                .map(|r| r.repository_id.as_str())
                .collect::<BTreeSet<_>>()
                == expected,
        "graph_repository_set_changed"
    );
    for input in &inputs.repositories {
        for file in [&input.preparation, &input.core, &input.pointer] {
            ensure!(
                bind_file(&file.path, Some(&file.sha256))? == *file,
                "graph_input_changed"
            );
        }
        let wal = PathBuf::from(format!("{}-wal", input.core.path.display()));
        ensure!(
            !wal.exists() || fs::metadata(wal)?.len() == 0,
            "unbound_graph_wal"
        );
        if job.operation == "export_graphs" {
            ensure!(
                job.graph_preparations.get(&input.repository_id) == Some(&input.preparation),
                "unregistered_graph_preparation"
            );
        }
    }
    if let Some(vectors) = &job.vectors {
        ensure!(
            vectors.sha256 == "7f604b30b823066bd5b0ed71106d10577c28495abd270444bc8ad5b7a63cb70a",
            "unregistered_vectors"
        );
    }
    if let Some(control) = &job.control_run {
        ensure!(
            control.sha256 == "c14da697d03707c0096f5f2fd7a97bff2ab5b4a6f9326c4a9a03da2066d545f2",
            "unregistered_control"
        );
    }
    Ok(inputs.repositories)
}

fn authenticate_graph_input(graph: &Graph, inputs: &[GraphInput]) -> Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    let input = inputs
        .iter()
        .find(|i| i.repository_id == graph.repository_id)
        .context("unregistered_graph")?;
    ensure!(
        graph.core == input.core
            && graph.preparation == input.preparation
            && graph.core_pointer == read_bound_json::<Value>(&input.pointer)?,
        "graph_authority_changed"
    );
    Ok(())
}

fn overlaps(fragment: &FrozenFragmentV1, path: &str, start: u32, end: u32) -> bool {
    fragment.path == path && fragment.line_range.start <= end && start <= fragment.line_range.end
}

fn graph_for(
    repository: &PreparedRepositoryV1,
    fragments: &[FrozenFragmentV1],
    binding: &FileBinding,
) -> Result<Graph> {
    let prepared: Value = read_bound_json(binding)?;
    ensure!(
        prepared["project_root"] == repository.local_root.to_string_lossy().as_ref(),
        "graph_root_mismatch"
    );
    let storage = PathBuf::from(
        prepared["storage_path"]
            .as_str()
            .context("graph_storage_missing")?,
    );
    let pin = CoreReadSession::pin(&storage)?;
    ensure!(
        serde_json::to_value(pin.pointer())? == prepared["core_pointer"],
        "graph_pointer_drift"
    );
    ensure!(
        prepared["publication"] == repository.publication,
        "graph_publication_mismatch"
    );
    ensure!(
        super::super::prepare::git_head(&repository.local_root)? == repository.commit,
        "graph_repository_drift"
    );
    let store = pin.storage();
    let core = bind_file(pin.generation_path(), None)?;
    let mut files = BTreeMap::new();
    let mut sources = BTreeMap::new();
    let mut source_bindings = Vec::new();
    for file in store.get_files()? {
        let source_path = if file.path.is_absolute() {
            file.path.clone()
        } else {
            repository.local_root.join(&file.path)
        };
        let canonical = fs::canonicalize(&source_path)?;
        ensure!(
            canonical.starts_with(&repository.local_root),
            "graph_source_escape"
        );
        let relative = canonical
            .strip_prefix(&repository.local_root)?
            .to_string_lossy()
            .to_string();
        let bytes = fs::read(&canonical)?;
        let digest = sha256(&bytes);
        ensure!(
            store.get_file_content_hash(file.id)?.as_deref() == Some(digest.as_str()),
            "graph_source_hash_drift:{}",
            relative
        );
        source_bindings.push(bind_file(&canonical, Some(&digest))?);
        // Non-UTF-8/missing-precision source cannot supply a positive witness.
        if let Ok(source) = String::from_utf8(bytes) {
            sources.insert(file.id, (relative.clone(), digest, source));
        }
        files.insert(file.id, relative);
    }
    let map_range = |path: &str, start: u32, end: u32| {
        fragments
            .iter()
            .filter(|f| f.project_id == repository.project_id && overlaps(f, path, start, end))
            .map(|f| f.fragment_id.clone())
            .collect::<Vec<_>>()
    };
    let mut nodes = Vec::new();
    for node in store.get_nodes()? {
        let ids = if node.kind != NodeKind::FILE {
            match (node.file_node_id, node.start_line, node.end_line) {
                (Some(file), Some(start), Some(end)) if start > 0 && end >= start => files
                    .get(&file.0)
                    .map(|path| map_range(path, start, end))
                    .unwrap_or_default(),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        nodes.push(GraphNode {
            id: node.id.0,
            fragment_ids: ids,
        });
    }
    nodes.sort_by_key(|n| n.id);
    let node_ids = nodes.iter().map(|n| n.id).collect::<BTreeSet<_>>();
    let mut relations = Vec::new();
    let mut gaps = Vec::new();
    for edge in store.get_edges()? {
        if edge.certainty != Some(ResolutionCertainty::Certain) {
            continue;
        }
        let source = edge.effective_source().0;
        let target = edge.effective_target().0;
        if !node_ids.contains(&source) || !node_ids.contains(&target) {
            gaps.push(json!({"edge_id": edge.id.0,"kind":"missing_effective_endpoint"}));
            continue;
        }
        let mut locations = match (edge.file_node_id, edge.line) {
            (Some(file), Some(line)) if line > 0 => vec![(file.0, line, line)],
            _ => Vec::new(),
        };
        if locations.is_empty() {
            for occurrence in store.get_occurrences_for_element(edge.id.0)? {
                if matches!(
                    occurrence.kind,
                    codestory_contracts::graph::OccurrenceKind::REFERENCE
                        | codestory_contracts::graph::OccurrenceKind::MACRO_REFERENCE
                ) {
                    locations.push((
                        occurrence.location.file_node_id.0,
                        occurrence.location.start_line,
                        occurrence.location.end_line,
                    ));
                }
            }
        }
        locations.sort();
        locations.dedup();
        let witnesses = locations
            .into_iter()
            .filter_map(|(file, start, end)| {
                let (path, digest, text) = sources.get(&file)?;
                let lines = text.split_inclusive('\n').collect::<Vec<_>>();
                if start == 0 || end < start || end as usize > lines.len() {
                    return None;
                }
                let source = lines[start as usize - 1..end as usize].concat();
                if source.trim().is_empty() {
                    return None;
                }
                Some(Witness {
                    path: path.clone(),
                    start_line: start,
                    end_line: end,
                    content_digest: digest.clone(),
                    source,
                })
            })
            .collect::<Vec<_>>();
        if witnesses.is_empty() {
            gaps.push(json!({"edge_id":edge.id.0,"kind":"missing_positive_occurrence"}));
            continue;
        }
        for occurrence in witnesses {
            let occurrence_fragment_ids =
                map_range(&occurrence.path, occurrence.start_line, occurrence.end_line);
            relations.push(Relation {
                id: format!(
                    "{}:{}:{}:{}",
                    edge.id.0, occurrence.path, occurrence.start_line, occurrence.end_line
                ),
                source,
                target,
                certainty: "Certain".into(),
                occurrence,
                occurrence_fragment_ids,
                raw: serde_json::to_value(&edge)?,
            });
        }
    }
    relations.sort_by(|a, b| a.id.cmp(&b.id));
    ensure!(
        bind_file(pin.generation_path(), Some(&core.sha256))? == core,
        "graph_core_changed"
    );
    Ok(Graph {
        repository_id: repository.repository_id.clone(),
        preparation: binding.clone(),
        core,
        core_pointer: prepared["core_pointer"].clone(),
        source_bindings,
        nodes,
        relations,
        gaps,
    })
}

pub fn frontier(
    graph: &Graph,
    seeds: &[String],
    scores: &HashMap<String, f32>,
) -> Result<(Vec<String>, Vec<Step>)> {
    ensure!(seeds.len() <= SEED_LIMIT, "structural_seed_budget");
    let nodes = graph
        .nodes
        .iter()
        .map(|n| (n.id, n))
        .collect::<HashMap<_, _>>();
    let seeds_set = seeds.iter().cloned().collect::<BTreeSet<_>>();
    let mut prior = BTreeSet::new();
    let mut successors = Vec::new();
    let mut steps = Vec::new();
    for seed in seeds {
        let anchors = graph
            .nodes
            .iter()
            .filter(|n| n.fragment_ids.contains(seed))
            .map(|n| n.id)
            .collect::<BTreeSet<_>>();
        let relations = graph
            .relations
            .iter()
            .filter(|e| {
                anchors.contains(&e.source)
                    || anchors.contains(&e.target)
                    || e.occurrence_fragment_ids.contains(seed)
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut eligible = BTreeSet::new();
        let mut gaps = Vec::new();
        for edge in &relations {
            ensure!(
                edge.certainty == "Certain" && !edge.occurrence.source.trim().is_empty(),
                "structural_edge_authority"
            );
            for id in [edge.source, edge.target] {
                let node = nodes.get(&id).context("structural_endpoint_missing")?;
                if node.fragment_ids.is_empty() {
                    gaps.push(json!({"relation_id":edge.id,"node_id":id,"kind":"endpoint_outside_fragment_universe"}));
                }
                eligible.extend(node.fragment_ids.iter().cloned());
            }
            if edge.occurrence_fragment_ids.is_empty() {
                gaps.push(
                    json!({"relation_id":edge.id,"kind":"occurrence_outside_fragment_universe"}),
                );
            }
            eligible.extend(edge.occurrence_fragment_ids.iter().cloned());
        }
        let excluded = seeds_set.union(&prior).cloned().collect::<BTreeSet<_>>();
        let mut ranked = eligible
            .difference(&excluded)
            .map(|id| {
                let score = *scores.get(id).context("structural_score_missing")?;
                ensure!(score.is_finite(), "structural_nonfinite_score");
                Ok((id.clone(), score))
            })
            .collect::<Result<Vec<_>>>()?;
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let retained = ranked
            .into_iter()
            .take(SUCCESSORS_PER_QUERY)
            .map(|p| p.0)
            .collect::<Vec<_>>();
        prior.extend(retained.iter().cloned());
        successors.extend(retained.iter().cloned());
        steps.push(Step {
            seed_fragment_id: seed.clone(),
            anchors: anchors.into_iter().collect(),
            relations,
            eligible: eligible.into_iter().collect(),
            excluded_before: excluded.into_iter().collect(),
            retained_successors: retained,
            boundary_gaps: gaps,
        });
    }
    ensure!(
        successors.len() <= MAX_SUCCESSORS,
        "structural_successor_budget"
    );
    Ok((successors, steps))
}

pub fn execute(job_path: &Path) -> Result<()> {
    let job_binding = bind_file(job_path, None)?;
    let job: Job = read_bound_json(&job_binding)?;
    let control = RunControl::new(&job.cancel_file)?;
    let preparation: Etr1PreparationV1 = read_bound_json(&job.preparation)?;
    let graph_inputs = fixed_inputs(&job, &preparation)?;
    let build = build_identity()?;
    ensure!(!build.source_dirty, "dirty_structural_binary");
    bind_file(&job.method.path, Some(&job.method.sha256))?;
    let stage = stage_output_directory(&job.output)?;
    if job.operation == "index_canary" {
        ensure!(
            preparation.authority == "synthetic_canary_only" && preparation.repositories.len() == 1,
            "not_synthetic_canary"
        );
        use codestory_contracts::workspace::SourceIndexPolicy;
        use codestory_runtime::{
            RetrievalProcessDefaults, RetrievalRuntimeDefaults, RetrievalRuntimeOverrides, Runtime,
            RuntimeProcessConfig, RuntimeRetrievalConfig, RuntimeRetrievalProfile,
        };
        let repository = &preparation.repositories[0];
        let storage = job.output.join("codestory.db");
        // Publish the owned directory first because core pointers contain paths.
        publish_output_directory(stage, &job.output)?;
        let defaults = RetrievalProcessDefaults::new(
            job.output.join("runtime"),
            RetrievalRuntimeDefaults::default(),
        );
        let retrieval = RuntimeRetrievalConfig::for_project_profile_with_process_defaults(
            Some(&repository.local_root),
            RuntimeRetrievalProfile::Local,
            None,
            &defaults,
            &RetrievalRuntimeOverrides::default(),
        );
        let runtime =
            Runtime::new_with_process_config(RuntimeProcessConfig::new_with_retrieval_config(
                retrieval,
                SourceIndexPolicy::default(),
            ));
        runtime
            .project_service()
            .open_project_summary_with_storage_path(repository.local_root.clone(), storage.clone())
            .map_err(|e| anyhow::anyhow!(e.message))?;
        runtime
            .index_service()
            .run_indexing_blocking_without_runtime_refresh(
                codestory_contracts::api::IndexMode::Full,
            )
            .map_err(|e| anyhow::anyhow!(e.message))?;
        drop(runtime);
        let pin = CoreReadSession::pin(&storage)?;
        write_exclusive(
            &job.output.join("prepared.json"),
            &serialize_pretty(&json!({"project_root":repository.local_root,
            "storage_path":storage,"core_pointer":pin.pointer(),"publication":repository.publication,"build":build}))?,
        )?;
        return Ok(());
    }
    if job.operation == "export_graphs" {
        let mut graphs = Vec::new();
        for repository in &preparation.repositories {
            control.check()?;
            let graph = graph_for(
                repository,
                &preparation.fragments,
                job.graph_preparations
                    .get(&repository.repository_id)
                    .context("graph_preparation_missing")?,
            )?;
            authenticate_graph_input(&graph, &graph_inputs)?;
            graphs.push(graph);
        }
        write_exclusive(
            &stage.path().join("graphs.json"),
            &serialize_pretty(&json!({"contract":"codestory.str1-graphs/v1",
            "build":build,"job":job_binding,"preparation":job.preparation,"method":job.method,"graphs":graphs,"annotation_access":"not_accessed"}))?,
        )?;
        control.check()?;
        return publish_output_directory(stage, &job.output);
    }
    ensure!(job.operation == "run", "unknown_structural_operation");
    let graph_binding = job.graphs.as_ref().context("graphs_missing")?;
    let graph_value: Value = read_bound_json(graph_binding)?;
    ensure!(
        graph_value["build"] == serde_json::to_value(&build)?
            && graph_value["preparation"] == serde_json::to_value(&job.preparation)?,
        "graph_build_or_preparation_changed"
    );
    let graphs: Vec<Graph> = serde_json::from_value(graph_value["graphs"].clone())?;
    let mut core_pins = Vec::new();
    for graph in &graphs {
        authenticate_graph_input(graph, &graph_inputs)?;
        let prepared: Value = read_bound_json(&graph.preparation)?;
        let pin = CoreReadSession::pin(Path::new(
            prepared["storage_path"]
                .as_str()
                .context("graph_storage_missing")?,
        ))?;
        ensure!(
            serde_json::to_value(pin.pointer())? == graph.core_pointer
                && pin.generation_path() == graph.core.path,
            "graph_publication_drift"
        );
        core_pins.push(pin);
        bind_file(&graph.core.path, Some(&graph.core.sha256))?;
        for file in &graph.source_bindings {
            bind_file(&file.path, Some(&file.sha256))?;
        }
    }
    let vector_binding = job.vectors.as_ref().context("vectors_missing")?;
    let (_, vector_artifact, vectors) =
        load_vector_artifact(&vector_binding.path, &vector_binding.sha256, &preparation)?;
    let control_binding = job.control_run.as_ref().context("control_run_missing")?;
    let frozen: Etr1RunManifestV1 = read_bound_json(control_binding)?;
    ensure!(
        frozen.preparation == job.preparation && frozen.fragment_vectors == *vector_binding,
        "frozen_control_input_mismatch"
    );
    let frozen_rows = frozen
        .rows
        .iter()
        .map(read_bound_json::<Value>)
        .collect::<Result<Vec<_>>>()?;
    let runtime = SidecarRuntimeConfig::local();
    let events_path =
        validate_isolated_state(job.state_root.as_ref().context("state_missing")?, &runtime)?;
    codestory_cli::install_native_embedding_client_transport()?;
    let mut residency = PerUserEmbeddingClient::for_runtime(&runtime)?.acquire_residency_lease()?;
    let initial_engine = engine_receipt(residency.identity())?;
    for key in ["model_digest", "ggml_build_identity"] {
        ensure!(
            initial_engine[key] == frozen.initial_engine[key]
                && initial_engine[key] == vector_artifact.initial_engine[key],
            "structural_cross_engine_mismatch:{key}"
        );
    }
    let client = ProductEmbeddingClient::new(&runtime);
    let fragments = preparation
        .fragments
        .iter()
        .map(|f| (f.fragment_id.clone(), f))
        .collect::<HashMap<_, _>>();
    let lexical = preparation
        .repositories
        .iter()
        .map(|r| {
            Ok((
                r.repository_id.clone(),
                Etr1LexicalIndex::new(
                    r.fragment_ids
                        .iter()
                        .map(|id| fragments[id].source.as_str()),
                )?,
            ))
        })
        .collect::<Result<HashMap<_, _>>>()?;
    let mut cursor = EventCursor::default();
    let mut batch_ordinal = 0;
    let mut rows = Vec::new();
    for wording in &preparation.wordings {
        control.check()?;
        let started = Instant::now();
        let repository = preparation
            .repositories
            .iter()
            .find(|r| r.repository_id == wording.repository_id)
            .context("repository_missing")?;
        let graph = graphs
            .iter()
            .find(|g| g.repository_id == wording.repository_id)
            .context("graph_missing")?;
        let phase = Instant::now();
        let (_, matches) = lexical[&repository.repository_id].search(&wording.question)?;
        let seeds = natural_seed_prefix(&matches)
            .iter()
            .map(|m| repository.fragment_ids[m.rowid - 1].clone())
            .collect::<Vec<_>>();
        ensure!(seeds == wording.seed_fragment_ids, "structural_seed_drift");
        let bm25 = phase.elapsed().as_nanos() as u64;
        let mut authenticator = SourceAuthenticator::new(repository, &fragments);
        let phase = Instant::now();
        for id in &seeds {
            authenticator.authenticate(id)?;
        }
        let seed_auth = phase.elapsed().as_nanos() as u64;
        let phase = Instant::now();
        let (query_vector, batches) = if let Some(seed) = seeds.first() {
            let mut specs = vec![QuerySpec {
                ordinal: 0,
                seed_fragment_id: seed.clone(),
                original_input: wording.question.clone(),
                encoded_input: wording.question.clone(),
                removed_trailing_source_lines: 0,
                model_limit_rejections: 0,
            }];
            let (encoded, batches) = encode_queries(
                &control,
                &client,
                "structural",
                &mut specs,
                &[wording.question.clone()],
                &[String::new()],
                &events_path,
                &mut cursor,
                &mut batch_ordinal,
            )?;
            ensure!(
                encoded[0].spec.encoded_input == wording.question
                    && encoded[0].spec.model_limit_rejections == 0,
                "raw_query_changed"
            );
            (encoded[0].vector.clone(), batches)
        } else {
            (Vec::new(), Vec::new())
        };
        let encoding = phase.elapsed().as_nanos() as u64;
        let phase = Instant::now();
        let scores = if seeds.is_empty() {
            Vec::new()
        } else {
            score_fragments(&query_vector, &repository.fragment_ids, &vectors)?.0
        };
        let score_map = repository
            .fragment_ids
            .iter()
            .cloned()
            .zip(scores.iter().copied())
            .collect::<HashMap<_, _>>();
        let (successors, steps) = frontier(graph, &seeds, &score_map)?;
        let discovery = phase.elapsed().as_nanos() as u64;
        let phase = Instant::now();
        let mut pool = seeds.clone();
        pool.extend(successors.iter().cloned());
        let legal = exact_legally_selectable_pool(&pool, repository, &fragments)?;
        let mapping = phase.elapsed().as_nanos() as u64;
        let phase = Instant::now();
        for id in &successors {
            authenticator.authenticate(id)?;
        }
        let hydration = phase.elapsed().as_nanos() as u64;
        let wall = started.elapsed().as_nanos() as u64;
        let old = frozen_rows
            .iter()
            .position(|r| {
                r["case_id"] == wording.case_id && r["phrasing_id"] == wording.phrasing_id
            })
            .context("frozen_control_row_missing")?;
        ensure!(
            frozen_rows[old]["seed_fragment_ids"] == serde_json::to_value(&seeds)?,
            "control_seed_mismatch"
        );
        rows.push(json!({"case_id":wording.case_id,"phrasing_id":wording.phrasing_id,"group":wording.group,"repository_id":wording.repository_id,
            "question_sha256":wording.question_sha256,"seed_fragment_ids":seeds,"control_row":frozen.rows[old],
            "candidate":{"legally_selectable_pool":legal,"descriptor_pool":pool,"hydrated_pool":pool,"successors":successors,
            "steps":steps,"query_input":wording.question,"query_vector":query_vector,"scores":scores,"batch_receipts":batches,
            "source_authentication":authenticator.receipt,"timing":{"round_zero_bm25_ns":bm25,"seed_source_authentication_ns":seed_auth,
            "query_encoding_ns":encoding,"structural_discovery_and_scoring_ns":discovery,"descriptor_mapping_ns":mapping,
            "remaining_source_authentication_ns":hydration,"prepared_state_ns":wall,"unaccounted_ns":wall-(bm25+seed_auth+encoding+discovery+mapping+hydration)}}}));
    }
    control.check()?;
    let final_engine = engine_receipt(&residency.revalidate()?)?;
    ensure!(
        initial_engine["server_instance_id"] == final_engine["server_instance_id"]
            && initial_engine["load_generation"] == final_engine["load_generation"],
        "structural_engine_drift"
    );
    let events = fs::read(&events_path)?;
    ensure!(
        read_completed_events(&events_path)?.len() == cursor.completed_events,
        "structural_completion_mismatch"
    );
    write_exclusive(&stage.path().join("events.jsonl"), &events)?;
    write_exclusive(
        &stage.path().join("run.json"),
        &serialize_pretty(
            &json!({"contract":"codestory.str1-run/v1","build":build,"job":job_binding,
        "preparation":job.preparation,"method":job.method,"graphs":graph_binding,"vectors":vector_binding,"control_run":control_binding,
        "annotation_access":"not_accessed","experiment_status":"awaiting_validation","decision":"not_evaluated",
        "initial_engine":initial_engine,"final_engine":final_engine,"events_sha256":sha256(&events),"rows":rows}),
        )?,
    )?;
    control.check()?;
    publish_output_directory(stage, &job.output)
}
