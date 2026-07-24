use super::{
    ApiError, LlmSearchDoc, LlmSymbolDoc, RetrievalFileRole, SearchEngine, Storage,
    retrieval_file_role_from_path,
};

pub(super) fn map_llm_doc_to_search(doc: LlmSymbolDoc) -> LlmSearchDoc {
    let file_role = doc
        .file_path
        .as_deref()
        .map(retrieval_file_role_from_path)
        .unwrap_or(RetrievalFileRole::Source);
    LlmSearchDoc {
        node_id: doc.node_id,
        file_role,
        doc_text: doc.doc_text,
        embedding: doc.embedding,
    }
}

pub(super) fn reload_llm_docs_from_storage(
    storage: &Storage,
    engine: &mut SearchEngine,
    batch_size: usize,
) -> Result<(), ApiError> {
    engine.clear_llm_symbol_docs();
    let mut after_node_id = None;
    let batch_size = batch_size.max(1);
    loop {
        let docs = storage
            .get_llm_symbol_docs_batch_after(after_node_id, batch_size)
            .map_err(|error| {
                ApiError::internal(format!("Failed to load LLM symbol docs: {error}"))
            })?;
        if docs.is_empty() {
            break;
        }
        after_node_id = docs.last().map(|doc| doc.node_id);
        engine.extend_llm_symbol_docs(docs.into_iter().map(map_llm_doc_to_search));
    }
    Ok(())
}
