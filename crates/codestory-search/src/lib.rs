use anyhow::{Context, Result};
use codestory_core::NodeId;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32String};
use std::collections::HashSet;
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::doc;
use tantivy::query::QueryParser;
use tantivy::schema::{FAST, INDEXED, STORED, Schema, TEXT, Value};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

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
    pub fn new(storage_path: Option<&Path>) -> Result<Self> {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("name", TEXT | STORED);
        schema_builder.add_i64_field("node_id", INDEXED | STORED | FAST);
        let schema = schema_builder.build();

        let index = if let Some(path) = storage_path {
            std::fs::create_dir_all(path)?;
            match Index::open_in_dir(path) {
                Ok(index) => index,
                Err(open_err) => Index::create_in_dir(path, schema.clone()).with_context(|| {
                    format!(
                        "Failed to open existing tantivy index at {}: {open_err}",
                        path.display()
                    )
                })?,
            }
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
        let mut index_writer: IndexWriter<TantivyDocument> = self.index.writer(50_000_000)?;
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
        self.search_symbol_with_scores(query)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    pub fn search_symbol_with_scores(&mut self, query: &str) -> Vec<(NodeId, f32)> {
        if query.is_empty() {
            return Vec::new();
        }

        let pattern = Pattern::new(
            query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );

        let mut matches = Vec::new();

        for (name, id) in &self.symbols {
            if let Some(score) = pattern.score(name.slice(..), &mut self.matcher) {
                matches.push((*id, score));
            }
        }

        matches.sort_by_key(|b| std::cmp::Reverse(b.1));

        let mut seen = HashSet::new();
        matches
            .into_iter()
            .map(|(id, score)| (id, score as f32))
            .filter(|(id, _)| seen.insert(*id))
            .take(20)
            .collect()
    }

    /// Remove symbols from both fuzzy and full-text projections.
    pub fn remove_nodes(&mut self, nodes: &[NodeId]) -> Result<()> {
        if nodes.is_empty() {
            return Ok(());
        }

        let mut remove_ids = HashSet::new();
        for id in nodes {
            remove_ids.insert(id.0);
        }

        self.symbols.retain(|(_, id)| !remove_ids.contains(&id.0));

        let mut index_writer: IndexWriter<TantivyDocument> = self.index.writer(50_000_000)?;
        let schema = self.index.schema();
        let node_field = schema.get_field("node_id")?;
        for id in &remove_ids {
            index_writer.delete_term(Term::from_field_i64(node_field, *id));
        }
        index_writer.commit()?;
        self.reader.reload()?;
        Ok(())
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

    #[test]
    fn test_remove_nodes() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;

        engine.index_nodes(vec![
            (NodeId(1), "AlphaSymbol".to_string()),
            (NodeId(2), "BetaSymbol".to_string()),
            (NodeId(3), "GammaSymbol".to_string()),
        ])?;

        let before = engine.search_symbol("Beta");
        assert!(before.contains(&NodeId(2)));
        assert_eq!(engine.search_full_text("betasymbol")?, vec![NodeId(2)]);

        engine.remove_nodes(&[NodeId(2)])?;

        let after = engine.search_symbol("Beta");
        assert!(!after.contains(&NodeId(2)));
        assert!(engine.search_full_text("betasymbol")?.is_empty());

        let remaining = engine.search_symbol("Gamma");
        assert!(remaining.contains(&NodeId(3)));

        Ok(())
    }
}
