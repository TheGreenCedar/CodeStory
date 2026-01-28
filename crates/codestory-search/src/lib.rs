use anyhow::{Context, Result};
use codestory_core::NodeId;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32String};
use std::collections::HashSet;
use tantivy::collector::TopDocs;
use tantivy::doc;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{Index, IndexReader, ReloadPolicy, TantivyDocument};

pub struct SearchEngine {
    // Nucleo matcher for fuzzy symbol search
    matcher: Matcher,
    // Symbol cache for nucleo: (normalized_name, node_id)
    symbols: Vec<(Utf32String, NodeId)>,

    // Tantivy for full-text search
    index: Index,
    reader: IndexReader,
}

impl SearchEngine {
    pub fn new(storage_path: Option<&std::path::Path>) -> Result<Self> {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("name", TEXT | STORED);
        schema_builder.add_i64_field("node_id", INDEXED | STORED | FAST);
        let schema = schema_builder.build();

        let index = if let Some(path) = storage_path {
            std::fs::create_dir_all(path)?;
            Index::open_in_dir(path).unwrap_or_else(|_| {
                Index::create_in_dir(path, schema.clone()).expect("Failed to create tantivy index")
            })
        } else {
            Index::create_in_ram(schema)
        };

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;

        Ok(Self {
            matcher: Matcher::new(Config::DEFAULT),
            symbols: Vec::new(),
            index,
            reader,
        })
    }

    /// Index a batch of nodes for both fuzzy and full-text search
    pub fn index_nodes(&mut self, nodes: Vec<(NodeId, String)>) -> Result<()> {
        let mut index_writer = self.index.writer(50_000_000)?;
        let schema = self.index.schema();
        let name_field = schema.get_field("name")?;
        let id_field = schema.get_field("node_id")?;

        for (id, name) in nodes {
            // Add to nucleo symbols
            self.symbols.push((Utf32String::from(name.as_str()), id));

            // Add to tantivy
            index_writer.add_document(doc!(
                name_field => name,
                id_field => id.0
            ))?;
        }

        index_writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    /// Fuzzy symbol search using nucleo-matcher
    pub fn search_symbol(&mut self, query: &str) -> Vec<NodeId> {
        if query.is_empty() {
            return Vec::new();
        }

        // Use Pattern API which properly handles edge cases
        let pattern = Pattern::new(
            query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );

        let mut matches = Vec::new();

        // Match against all symbols
        for (name, id) in &self.symbols {
            if let Some(score) = pattern.score(name.slice(..), &mut self.matcher) {
                matches.push((*id, score));
            }
        }

        // Sort by score descending
        matches.sort_by(|a, b| b.1.cmp(&a.1));

        let mut seen = HashSet::new();
        matches
            .into_iter()
            .filter(|(id, _)| seen.insert(*id))
            .map(|(id, _)| id)
            .take(20)
            .collect()
    }

    /// Full-text search using tantivy
    pub fn search_full_text(&self, query_str: &str) -> Result<Vec<NodeId>> {
        if query_str.is_empty() {
            return Ok(Vec::new());
        }

        let searcher = self.reader.searcher();
        let schema = self.index.schema();
        let name_field = schema.get_field("name")?;
        let id_field = schema.get_field("node_id")?;

        let query_parser = QueryParser::for_index(&self.index, vec![name_field]);
        let query = query_parser
            .parse_query(query_str)
            .context("Failed to parse tantivy query")?;

        let top_docs = searcher.search(&query, &TopDocs::with_limit(20))?;

        let mut results = Vec::new();
        let mut seen = HashSet::new();
        for (_score, doc_address) in top_docs {
            let retrieved_doc: TantivyDocument = searcher.doc(doc_address)?;
            if let Some(id_val) = retrieved_doc.get_first(id_field).and_then(|v| v.as_i64()) {
                let id = NodeId(id_val);
                if seen.insert(id) {
                    results.push(id);
                }
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_engine() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;

        let nodes = vec![
            (NodeId(1), "MyClass".to_string()),
            (NodeId(2), "my_function".to_string()),
            (NodeId(3), "another_function".to_string()),
        ];

        engine.index_nodes(nodes)?;

        // Test fuzzy search - "MyClass" should be the top result for "MyC"
        let results = engine.search_symbol("MyC");
        assert!(!results.is_empty(), "Should find at least one match");
        assert_eq!(
            results[0],
            NodeId(1),
            "MyClass should be the best match for 'MyC'"
        );

        // Fuzzy search for "func" should match both function names
        let results = engine.search_symbol("func");
        assert_eq!(results.len(), 2); // my_function, another_function

        // Test full-text search
        let results = engine.search_full_text("another")?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], NodeId(3));

        Ok(())
    }
}
