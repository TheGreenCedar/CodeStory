use super::engine::{HybridSearchConfig, HybridSearchHit, HybridSearchState, SearchEngine};
use super::lexical::exact_symbol_merged_lexical_hybrid_hits_for_symbols;
use anyhow::Result;
use codestory_contracts::graph::NodeId;
use std::collections::HashMap;

/// Immutable search index safe to share across parallel packet subqueries.
#[derive(Clone)]
pub struct SearchEngineSnapshot {
    hybrid: HybridSearchState,
    version: u64,
}

impl SearchEngineSnapshot {
    pub fn version_from_engine(engine: &SearchEngine) -> u64 {
        engine.snapshot_content_version()
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn from_engine(engine: &SearchEngine) -> Self {
        Self {
            hybrid: HybridSearchState::from_engine(engine),
            version: Self::version_from_engine(engine),
        }
    }

    pub fn exact_symbol_merged_lexical_hybrid_hits(
        &self,
        query: &str,
        graph_boosts: &HashMap<NodeId, f32>,
    ) -> Vec<HybridSearchHit> {
        exact_symbol_merged_lexical_hybrid_hits_for_symbols(
            self.hybrid.symbols(),
            query,
            graph_boosts,
        )
    }

    #[allow(dead_code)]
    pub fn search_hybrid_with_query_embedding(
        &self,
        query: &str,
        query_embedding: Option<&[f32]>,
        graph_boosts: &HashMap<NodeId, f32>,
        config: HybridSearchConfig,
    ) -> Result<Vec<HybridSearchHit>> {
        self.hybrid
            .search_hybrid_with_query_embedding(query, query_embedding, graph_boosts, config)
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use codestory_contracts::graph::NodeId;
    use rayon::prelude::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn snapshot_version_changes_when_symbol_table_changes() {
        let mut engine = SearchEngine::new(None).expect("search engine");
        engine.load_symbol_projection(vec![(NodeId(1), "Alpha".to_string())]);
        let first = SearchEngineSnapshot::from_engine(&engine);
        engine.load_symbol_projection(vec![
            (NodeId(1), "Alpha".to_string()),
            (NodeId(2), "Beta".to_string()),
        ]);
        let second = SearchEngineSnapshot::from_engine(&engine);
        assert_ne!(first.version(), second.version());
    }

    #[test]
    fn snapshot_supports_concurrent_symbol_reads() {
        let mut engine = SearchEngine::new(None).expect("search engine");
        engine.load_symbol_projection(vec![
            (NodeId(1), "run_index".to_string()),
            (NodeId(2), "WorkspaceIndexer".to_string()),
        ]);
        let snapshot = Arc::new(SearchEngineSnapshot::from_engine(&engine));
        let first = thread::spawn({
            let snapshot = Arc::clone(&snapshot);
            move || snapshot.exact_symbol_merged_lexical_hybrid_hits("run_index", &HashMap::new())
        });
        let second = thread::spawn({
            let snapshot = Arc::clone(&snapshot);
            move || {
                snapshot
                    .exact_symbol_merged_lexical_hybrid_hits("WorkspaceIndexer", &HashMap::new())
            }
        });
        assert!(!first.join().expect("first thread").is_empty());
        assert!(!second.join().expect("second thread").is_empty());
    }

    #[test]
    fn parallel_lexical_batch_matches_serial_symbol_scan() {
        let mut engine = SearchEngine::new(None).expect("search engine");
        let symbols = (0..2048)
            .map(|index| (NodeId(index + 1), format!("Symbol{index}Component")))
            .collect::<Vec<_>>();
        engine.load_symbol_projection(symbols);
        let snapshot = Arc::new(SearchEngineSnapshot::from_engine(&engine));
        let queries: Vec<String> = (0..8)
            .map(|index| format!("Symbol{}Component", index * 250))
            .collect();
        let graph_boosts = HashMap::new();

        let serial = queries
            .iter()
            .map(|query| snapshot.exact_symbol_merged_lexical_hybrid_hits(query, &graph_boosts))
            .collect::<Vec<_>>();
        let parallel = queries
            .par_iter()
            .map(|query| snapshot.exact_symbol_merged_lexical_hybrid_hits(query, &graph_boosts))
            .collect::<Vec<_>>();

        let project = |batches: Vec<Vec<HybridSearchHit>>| {
            batches
                .into_iter()
                .map(|hits| {
                    hits.into_iter()
                        .map(|hit| {
                            (
                                hit.node_id,
                                hit.lexical_score,
                                hit.semantic_score,
                                hit.graph_score,
                                hit.total_score,
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(project(parallel), project(serial));
    }
}
