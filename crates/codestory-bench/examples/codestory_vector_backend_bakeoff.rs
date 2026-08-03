//! Runner for the immutable vector-backend bake-off (W6.8, issue #1664).
//!
//! Publishes one immutable vector generation per rung of the declared ladder
//! through the *product's* attested publication path, then scores each
//! candidate backend against the *product's* dense scan over that same
//! generation. Nothing here reimplements the incumbent: the incumbent column of
//! the report is `codestory_retrieval`'s own `search_database`, reached through
//! the crate's `benchmark-support` surface, so an incumbent number that looks
//! good cannot be an artefact of a friendlier copy.
//!
//! The runner never decides anything. It records what it measured and what it
//! could not measure, and hands both to
//! `codestory_bench::vector_backend_bakeoff::evaluate_disposition`, which is
//! fail-closed: a run like this one — synthetic vectors, one platform, two of
//! four candidates unbuildable — cannot produce an adoption however fast a
//! candidate looks.
//!
//! It is a Cargo *example*, not a `src/bin` target, on purpose: the benchmark
//! surface it needs arrives through `codestory-bench`'s dev-dependencies, and
//! `[dependencies]` there feeds the packaged qualification binary. Keeping the
//! runner out of that section is what stops the measurement surface reaching a
//! shipped artefact.
//!
//! ```text
//! cargo run -p codestory-bench --example codestory_vector_backend_bakeoff -- \
//!     --out benchmarks/release-evidence/vector-backend-bakeoff/<host>.json
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Parser;
use codestory_bench::vector_backend_bakeoff::{
    BAKEOFF_SCHEMA, BakeoffGates, BakeoffResult, CandidateId, CandidateMeasurement,
    CandidateOutcome, CorpusProvenance, NotMeasuredReason, WORKLOAD_LADDER, WorkloadMeasurement,
    evaluate_disposition,
};
use codestory_retrieval::benchmark_support::{
    BenchmarkVector, PublishedVectorGeneration, publish_vector_generation, read_published_vectors,
    scan_published_vectors, semantic_stage_budget_ms,
};

/// A natural-language query shape, so the budget read from the planner is the
/// one the semantic stage actually gets for the queries this lane serves.
const BUDGET_PROBE_QUERY: &str = "how does the request dispatcher choose a transport adapter";

#[derive(Debug, Parser)]
#[command(
    name = "codestory_vector_backend_bakeoff",
    about = "Compare vector index backends under the shipped immutable-generation contract"
)]
struct Args {
    /// Where the machine-readable result document is written.
    #[arg(long)]
    out: PathBuf,
    /// Workload rungs to measure. Defaults to the declared ladder.
    #[arg(long = "workload", value_delimiter = ',')]
    workloads: Vec<u64>,
    /// Queries per block.
    #[arg(long, default_value_t = 100)]
    queries: u64,
    /// Counterbalanced blocks per rung. #1202's design calls for six.
    #[arg(long, default_value_t = 6)]
    blocks: u64,
    /// Embedding width. Defaults to the shipped retrieval width.
    #[arg(long, default_value_t = codestory_retrieval::RETRIEVAL_EMBEDDING_DIM)]
    dim: usize,
    /// Seed for the deterministic synthetic corpus.
    #[arg(long, default_value_t = 0x5EED_1664)]
    seed: u64,
    /// Assert the corpus is representative (real embeddings over a real
    /// repository). Only set this when it is true: it is the flag that lets a
    /// run authorize a backend swap.
    #[arg(long, default_value_t = false)]
    representative_corpus: bool,
    /// Human-readable description of where the vectors came from.
    #[arg(long, default_value = "deterministic pseudo-random unit vectors")]
    corpus_detail: String,
    /// The exact command that produced this binary, recorded verbatim so a
    /// latency number can be read against the optimization level that produced
    /// it.
    #[arg(long, default_value = "unspecified")]
    build_invocation: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let workloads = if args.workloads.is_empty() {
        WORKLOAD_LADDER.to_vec()
    } else {
        args.workloads.clone()
    };
    let corpus_provenance = if args.representative_corpus {
        CorpusProvenance::Representative
    } else {
        CorpusProvenance::Synthetic
    };
    let budget_ms = semantic_stage_budget_ms(BUDGET_PROBE_QUERY)
        .context("the retrieval planner reports no semantic stage for a natural-language query")?;
    let gates = BakeoffGates::declared(budget_ms);

    let mut incumbent_workloads = Vec::new();
    let mut resident_workloads = Vec::new();
    for vectors in &workloads {
        let rung = measure_rung(&args, *vectors, budget_ms)?;
        incumbent_workloads.push(rung.incumbent);
        resident_workloads.push(rung.resident_matrix);
    }

    let platform = host_platform();
    let platforms = BTreeSet::from([platform.to_string()]);
    let mut outcomes = BTreeMap::new();
    outcomes.insert(
        CandidateId::INCUMBENT.as_str().to_string(),
        CandidateOutcome::Measured(CandidateMeasurement {
            corpus_provenance,
            platforms: platforms.clone(),
            workloads: incumbent_workloads,
        }),
    );
    outcomes.insert(
        CandidateId::ExactResidentMatrix.as_str().to_string(),
        CandidateOutcome::Measured(CandidateMeasurement {
            corpus_provenance,
            platforms,
            workloads: resident_workloads,
        }),
    );
    for (candidate, crate_name) in [
        (CandidateId::SqliteVec, "sqlite-vec"),
        (CandidateId::Usearch, "usearch"),
    ] {
        outcomes.insert(
            candidate.as_str().to_string(),
            CandidateOutcome::NotMeasured {
                reason: NotMeasuredReason::DependencyNotVendored,
                detail: format!(
                    "{crate_name} is not a workspace dependency and is not present under \
                     vendor/; the offline build contract forbids fetching it during a proof run, \
                     so no build, load, query, memory, or corruption number exists for it"
                ),
            },
        );
    }

    let disposition = evaluate_disposition(&gates, &outcomes);
    let result = BakeoffResult {
        schema: BAKEOFF_SCHEMA.to_string(),
        host: platform.to_string(),
        host_detail: format!(
            "{} {} / debug_assertions={} / built by: {}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            cfg!(debug_assertions),
            args.build_invocation
        ),
        recorded_at: chrono_free_utc_date(),
        embedding_dim: args.dim,
        corpus_provenance,
        corpus_detail: args.corpus_detail.clone(),
        gates,
        outcomes,
        disposition,
        limitations: limitations(
            corpus_provenance,
            platform,
            &workloads,
            cfg!(debug_assertions),
        ),
    };

    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent).context("create bake-off output directory")?;
    }
    let mut json = serde_json::to_string_pretty(&result).context("serialize bake-off result")?;
    json.push('\n');
    std::fs::write(&args.out, json)
        .with_context(|| format!("write bake-off result {}", args.out.display()))?;
    println!(
        "vector-backend-bakeoff: {} candidates, disposition {:?}, written to {}",
        result.outcomes.len(),
        result.disposition,
        args.out.display()
    );
    Ok(())
}

struct RungMeasurements {
    incumbent: WorkloadMeasurement,
    resident_matrix: WorkloadMeasurement,
}

fn measure_rung(args: &Args, vectors: u64, budget_ms: u64) -> Result<RungMeasurements> {
    let count = usize::try_from(vectors).context("workload size fits in usize")?;
    let corpus = synthetic_corpus(args.seed, count, args.dim);
    let queries = synthetic_queries(
        args.seed ^ 0xA5A5_A5A5,
        usize::try_from(args.queries).context("query count fits in usize")?,
        args.dim,
    );

    let root = tempfile::tempdir().context("create bake-off root")?;
    let generation = format!("bakeoff-{vectors}");
    let input_hash = format!("bakeoff-input-{vectors}-{}", args.seed);
    let build_started = Instant::now();
    let published = publish_vector_generation(
        root.path(),
        "bakeoff",
        &generation,
        &input_hash,
        args.dim,
        &corpus,
    )?;
    let publish_millis = build_started.elapsed().as_secs_f64() * 1_000.0;
    anyhow::ensure!(
        published.point_count() == vectors,
        "published {} vectors, expected {vectors}",
        published.point_count()
    );

    let top_k = codestory_bench::vector_backend_bakeoff::DEFAULT_TOP_K;

    // Ground truth once per rung, outside every timed region.
    let expected = queries
        .iter()
        .map(|query| exhaustive_served_set(&corpus, query, top_k))
        .collect::<Vec<_>>();

    let load_started = Instant::now();
    let resident = ResidentMatrix::load(&published, args.dim)?;
    let resident_build_millis = publish_millis + load_started.elapsed().as_secs_f64() * 1_000.0;

    let mut incumbent_samples = Vec::new();
    let mut incumbent_agreements = Vec::new();
    let mut resident_samples = Vec::new();
    let mut resident_agreements = Vec::new();

    // One discarded warm-up pass per candidate. Without it the first scan pays
    // for a cold page cache and lands in the percentiles as if it were a query.
    for query in queries.iter().take(3) {
        scan_published_vectors(&published, query, top_k, &|| false)?;
        resident.search(query, top_k);
    }

    // Counterbalanced within the block: the candidate that runs first
    // alternates per query, so neither backend systematically pays the cost of
    // warming the cache for the other. #1202 asked for six counterbalanced
    // blocks; `--blocks` sets how many are run.
    for block in 0..args.blocks {
        for (index, query) in queries.iter().enumerate() {
            let incumbent_first = (block as usize + index).is_multiple_of(2);
            let mut run_incumbent = || -> Result<()> {
                let started = Instant::now();
                let hits = scan_published_vectors(&published, query, top_k, &|| false)?;
                incumbent_samples.push(started.elapsed().as_secs_f64() * 1_000_000.0);
                incumbent_agreements.push(agreement(&expected[index], &hits));
                Ok(())
            };
            let run_resident = |samples: &mut Vec<f64>, agreements: &mut Vec<f64>| {
                let started = Instant::now();
                let hits = resident.search(query, top_k);
                samples.push(started.elapsed().as_secs_f64() * 1_000_000.0);
                agreements.push(agreement(&expected[index], &hits));
            };
            if incumbent_first {
                run_incumbent()?;
                run_resident(&mut resident_samples, &mut resident_agreements);
            } else {
                run_resident(&mut resident_samples, &mut resident_agreements);
                run_incumbent()?;
            }
        }
    }

    let sampled_queries = queries.len() as u64 * args.blocks;
    Ok(RungMeasurements {
        incumbent: summarize(
            published.point_count(),
            sampled_queries,
            &incumbent_samples,
            &incumbent_agreements,
            // The shipped scan streams rows out of SQLite and keeps only the
            // bounded top-k window, so it holds no vector data resident
            // between queries. Page cache the kernel keeps for the database
            // file is not the backend's working set and is reclaimable under
            // pressure.
            0,
            "streaming scan; no vector data held resident between queries",
            publish_millis,
            budget_ms,
        ),
        resident_matrix: summarize(
            published.point_count(),
            sampled_queries,
            &resident_samples,
            &resident_agreements,
            resident.resident_bytes(),
            "contiguous f32 matrix, per-row norms, and node identities held for the generation",
            resident_build_millis,
            budget_ms,
        ),
    })
}

/// The exact resident-matrix candidate.
///
/// Built from the bytes the published generation actually carries, read back
/// through the same validation the product applies, so the candidate does not
/// get a corpus the incumbent never saw.
struct ResidentMatrix {
    dim: usize,
    matrix: Vec<f32>,
    norms: Vec<f32>,
    node_ids: Vec<String>,
}

impl ResidentMatrix {
    fn load(published: &PublishedVectorGeneration, dim: usize) -> Result<Self> {
        let loaded = read_published_vectors(published, dim)?;
        let mut matrix = Vec::with_capacity(loaded.len() * dim);
        let mut norms = Vec::with_capacity(loaded.len());
        let mut node_ids = Vec::with_capacity(loaded.len());
        for vector in &loaded {
            matrix.extend_from_slice(&vector.vector);
            norms.push(norm(&vector.vector));
            node_ids.push(vector.node_id.clone());
        }
        Ok(Self {
            dim,
            matrix,
            norms,
            node_ids,
        })
    }

    fn resident_bytes(&self) -> u64 {
        (self.matrix.len() * std::mem::size_of::<f32>()
            + self.norms.len() * std::mem::size_of::<f32>()
            + self.node_ids.iter().map(String::len).sum::<usize>()) as u64
    }

    fn search(&self, query: &[f32], top_k: usize) -> Vec<(String, f32)> {
        let query_norm = norm(query);
        let mut scored = Vec::with_capacity(self.node_ids.len());
        for (index, node_id) in self.node_ids.iter().enumerate() {
            let row = &self.matrix[index * self.dim..(index + 1) * self.dim];
            let dot = row
                .iter()
                .zip(query)
                .map(|(left, right)| left * right)
                .sum::<f32>();
            scored.push((dot / (self.norms[index] * query_norm), node_id.clone()));
        }
        scored.sort_by(|left, right| {
            right
                .0
                .total_cmp(&left.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        scored.truncate(top_k);
        // A candidate for this lane has to serve what the lane serves,
        // including its abstention rule: without this the candidate would be
        // credited for a window the product would never have shown.
        retain_dense_evidence(&mut scored);
        scored
            .into_iter()
            .map(|(score, node_id)| (node_id, score))
            .collect()
    }
}

/// The shipped lane's dense-abstention rule, applied to a candidate's window.
///
/// Mirrors `retain_dense_evidence` in `codestory_retrieval::embedded_vector`:
/// drop everything at or below zero similarity, and everything below half the
/// window's own best similarity. Requires `scored` sorted descending.
fn retain_dense_evidence(scored: &mut Vec<(f32, String)>) {
    let Some(best) = scored.first().map(|entry| entry.0) else {
        return;
    };
    if best <= 0.0 {
        scored.clear();
        return;
    }
    let floor = best * 0.5;
    scored.retain(|entry| entry.0 > 0.0 && entry.0 >= floor);
}

/// The window the shipped lane would serve for `query`, computed exhaustively
/// and independently of any backend under measurement.
///
/// Ranking is exact over the whole corpus and the lane's own abstention rule is
/// applied, so a candidate is judged against what the product would have shown
/// rather than against another backend's opinion.
fn exhaustive_served_set(
    corpus: &[BenchmarkVector],
    query: &[f32],
    top_k: usize,
) -> BTreeSet<String> {
    let query_norm = norm(query);
    let mut exact = corpus
        .iter()
        .map(|vector| {
            let dot = vector
                .vector
                .iter()
                .zip(query)
                .map(|(left, right)| left * right)
                .sum::<f32>();
            (dot / (norm(&vector.vector) * query_norm), &vector.node_id)
        })
        .collect::<Vec<_>>();
    exact.sort_by(|left, right| right.0.total_cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    exact.truncate(top_k);
    let Some(best) = exact.first().map(|entry| entry.0) else {
        return BTreeSet::new();
    };
    if best <= 0.0 {
        return BTreeSet::new();
    }
    let floor = best * 0.5;
    exact
        .iter()
        .filter(|entry| entry.0 > 0.0 && entry.0 >= floor)
        .map(|entry| entry.1.clone())
        .collect()
}

/// Symmetric agreement between a candidate's served window and the exhaustive
/// window the shipped lane would have served.
///
/// `|served ∩ exact| / |served ∪ exact|`. Symmetric on purpose: a backend that
/// invents a neighbour has changed the packet's evidence just as surely as one
/// that loses a neighbour, and a recall-only measure scores the inventor 1.0.
fn agreement(expected: &BTreeSet<String>, hits: &[(String, f32)]) -> f64 {
    let served = hits
        .iter()
        .map(|(node_id, _)| node_id.clone())
        .collect::<BTreeSet<_>>();
    let union = expected.union(&served).count();
    if union == 0 {
        // Both sides abstained. Agreeing to serve nothing is agreement.
        return 1.0;
    }
    expected.intersection(&served).count() as f64 / union as f64
}

#[allow(clippy::too_many_arguments)]
fn summarize(
    vectors: u64,
    queries: u64,
    samples: &[f64],
    agreements: &[f64],
    resident_bytes: u64,
    resident_bytes_basis: &str,
    build_millis: f64,
    budget_ms: u64,
) -> WorkloadMeasurement {
    let budget_micros = budget_ms as f64 * 1_000.0;
    let timed_out = samples
        .iter()
        .filter(|sample| **sample > budget_micros)
        .count();
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    WorkloadMeasurement {
        vectors,
        queries,
        top_k_agreement: if agreements.is_empty() {
            0.0
        } else {
            agreements.iter().sum::<f64>() / agreements.len() as f64
        },
        stage_timeout_rate: if samples.is_empty() {
            1.0
        } else {
            timed_out as f64 / samples.len() as f64
        },
        resident_bytes,
        resident_bytes_basis: resident_bytes_basis.to_string(),
        build_millis,
        p50_scan_micros: percentile(&sorted, 0.50),
        p95_scan_micros: percentile(&sorted, 0.95),
        max_scan_micros: sorted.last().copied().unwrap_or(0.0),
    }
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() as f64 - 1.0) * quantile).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn norm(vector: &[f32]) -> f32 {
    vector.iter().map(|value| value * value).sum::<f32>().sqrt()
}

/// Deterministic unit vectors. Same seed, same corpus, on any host.
fn synthetic_corpus(seed: u64, count: usize, dim: usize) -> Vec<BenchmarkVector> {
    let mut state = seed | 1;
    (0..count)
        .map(|index| BenchmarkVector {
            node_id: format!("bakeoff-node-{index:08}"),
            vector: unit_vector(&mut state, dim),
        })
        .collect()
}

fn synthetic_queries(seed: u64, count: usize, dim: usize) -> Vec<Vec<f32>> {
    let mut state = seed | 1;
    (0..count).map(|_| unit_vector(&mut state, dim)).collect()
}

fn unit_vector(state: &mut u64, dim: usize) -> Vec<f32> {
    let mut values = (0..dim)
        .map(|_| {
            // SplitMix64, then map into [-1, 1).
            *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = *state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            ((z >> 40) as f32 / 8_388_608.0) - 1.0
        })
        .collect::<Vec<f32>>();
    let magnitude = norm(&values);
    // A zero-norm vector is rejected by the product's publication validation,
    // which is the correct behaviour; give the generator a deterministic
    // fallback rather than emitting one.
    if magnitude <= f32::EPSILON {
        values[0] = 1.0;
        return values;
    }
    for value in &mut values {
        *value /= magnitude;
    }
    values
}

fn host_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "macos-arm64",
        ("macos", "x86_64") => "macos-x64",
        ("windows", "x86_64") => "windows-x64",
        ("windows", "aarch64") => "windows-arm64",
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        _ => "unknown",
    }
}

fn chrono_free_utc_date() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    // Civil-from-days, so the record carries a date without pulling a date
    // crate into the bench dependency graph.
    let days = (seconds / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

fn limitations(
    provenance: CorpusProvenance,
    platform: &str,
    workloads: &[u64],
    debug_assertions: bool,
) -> Vec<String> {
    let mut limitations = Vec::new();
    if debug_assertions {
        limitations.push(
            "Measured with debug assertions on. The shipping profile requires the embedded \
             model source, which this host does not have, so a true release-profile build was \
             not available. Absolute latencies are therefore pessimistic; they are recorded for \
             the cost model, not as a product latency claim."
                .to_string(),
        );
    }
    if !provenance.is_representative() {
        limitations.push(
            "Vectors are deterministic pseudo-random unit vectors, not embeddings of a real \
             repository. They exercise the scan's cost model at the declared widths and counts \
             and carry no answer-quality signal, so no recall claim from this run is \
             decision-grade."
                .to_string(),
        );
    }
    limitations.push(format!(
        "Only {platform} was exercised. Cross-platform immutable-generation behaviour on the \
         other shipped packages is unmeasured by this run."
    ));
    limitations.push(
        "sqlite-vec and USearch were not built. Neither is a workspace dependency nor vendored, \
         and the offline build contract forbids fetching a native dependency during a proof run."
            .to_string(),
    );
    limitations.push(
        "Absolute latencies carry the host's own noise. A run on a shared developer workstation \
         competes with whatever else that machine is doing; read the ordering and the scaling \
         with corpus size, and treat a rung that is faster than a smaller rung as evidence the \
         host was contended rather than as a property of the backend."
            .to_string(),
    );
    limitations.push(
        "Scan latency excludes query embedding. The semantic stage budget read from the \
         retrieval planner covers embedding plus scan, so the timeout rate reported here is a \
         lower bound on the stage's real timeout rate."
            .to_string(),
    );
    if workloads != WORKLOAD_LADDER {
        limitations.push(format!(
            "Measured rungs {workloads:?} are not the declared ladder {WORKLOAD_LADDER:?}."
        ));
    }
    limitations
}
