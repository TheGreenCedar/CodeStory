use crate::config::{SidecarLayout, ZOEKT_HEALTH_BUDGET};
use crate::outbound_http::read_text;
use crate::zoekt_index::{search_lexical_index, shard_dir_for};
use anyhow::Result;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ZoektHealthProbe {
    pub reachable: bool,
    pub latency_ms: u64,
    pub shard_count: u32,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct ZoektClient {
    base_url: String,
    timeout: Duration,
}

impl ZoektClient {
    pub fn new(layout: &SidecarLayout) -> Self {
        Self {
            base_url: layout.zoekt_base_url(),
            timeout: ZOEKT_HEALTH_BUDGET,
        }
    }

    pub fn health_probe(&self) -> ZoektHealthProbe {
        let started = Instant::now();
        let url = format!("{}/", self.base_url);
        match read_text(ureq::get(&url).timeout(self.timeout).call()) {
            Ok(response) => {
                let status = response.status;
                ZoektHealthProbe {
                    reachable: (200..500).contains(&status),
                    latency_ms: started.elapsed().as_millis() as u64,
                    shard_count: 0,
                    detail: format!("http {status}"),
                }
            }
            Err(error) => ZoektHealthProbe {
                reachable: false,
                latency_ms: started.elapsed().as_millis() as u64,
                shard_count: 0,
                detail: error.to_string(),
            },
        }
    }

    /// Lexical search: per-project shard only.
    ///
    /// The Zoekt HTTP `/search` API is intentionally not used for served hits until
    /// the real sidecar path carries a project/repo filter. Health probing can still
    /// prove the service is up, but primary retrieval must not leak global results.
    pub fn search(
        &self,
        layout: &SidecarLayout,
        project_id: &str,
        sidecar_input_hash: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<super::CandidateHit>> {
        use super::candidate::{CandidateHit, CandidateSource};
        let shard_dir = shard_dir_for(&layout.zoekt_data_dir, project_id);
        let hits = search_lexical_index(&shard_dir, sidecar_input_hash, query, limit)?
            .into_iter()
            .map(|hit| {
                let mut candidate = CandidateHit::with_source(
                    hit.path,
                    hit.symbol_name,
                    hit.score,
                    CandidateSource::Zoekt,
                );
                candidate.node_id = hit.node_id;
                candidate.start_line = hit.start_line;
                candidate.add_provenance(hit.source.provenance_label());
                candidate
            })
            .collect::<Vec<_>>();
        Ok(hits)
    }

    /// Probe search used by health to verify repo-relative hits exist.
    pub fn probe_lexical_search(
        &self,
        layout: &SidecarLayout,
        project_id: &str,
        sidecar_input_hash: &str,
    ) -> Result<Vec<String>> {
        let hits = self.search(layout, project_id, sidecar_input_hash, "fn", 4)?;
        Ok(hits.into_iter().map(|hit| hit.file_path).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zoekt_index::{build_zoekt_shard, lexical_input_fingerprint};
    use tempfile::TempDir;

    #[test]
    fn search_uses_project_local_shard_only() {
        let project_a = TempDir::new().expect("project a");
        let project_b = TempDir::new().expect("project b");
        std::fs::create_dir_all(project_a.path().join("src")).expect("mkdir a");
        std::fs::create_dir_all(project_b.path().join("src")).expect("mkdir b");
        std::fs::write(
            project_a.path().join("src").join("handler.rs"),
            "pub fn project_a_handler() {}",
        )
        .expect("write a");
        std::fs::write(
            project_b.path().join("src").join("handler.rs"),
            "pub fn project_b_handler() {}",
        )
        .expect("write b");

        let zoekt_data = TempDir::new().expect("zoekt data");
        let fingerprint_a = lexical_input_fingerprint(project_a.path(), None).expect("input a");
        build_zoekt_shard(
            project_a.path(),
            None,
            zoekt_data.path(),
            "project-a",
            &fingerprint_a,
            "input-a",
        )
        .expect("index a");
        let fingerprint_b = lexical_input_fingerprint(project_b.path(), None).expect("input b");
        build_zoekt_shard(
            project_b.path(),
            None,
            zoekt_data.path(),
            "project-b",
            &fingerprint_b,
            "input-b",
        )
        .expect("index b");

        let mut layout = SidecarLayout::from_env();
        layout.zoekt_data_dir = zoekt_data.path().to_path_buf();
        let client = ZoektClient::new(&layout);

        let hits = client
            .search(&layout, "project-a", "input-a", "project_b_handler", 10)
            .expect("search a");
        assert!(
            hits.is_empty(),
            "project-a search must not return project-b shard hits: {hits:?}"
        );
    }
}
