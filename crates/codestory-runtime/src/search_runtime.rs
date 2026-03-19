pub(crate) use crate::search::engine::SearchEngine;
pub use crate::search::engine::{
    DEFAULT_BUNDLED_EMBED_MODEL_PATH, EMBEDDING_MAX_TOKENS_ENV, EMBEDDING_MODEL_ENV,
    EMBEDDING_MODEL_ID_ENV, EMBEDDING_RUNTIME_MODE_ENV, EMBEDDING_TOKENIZER_ENV,
    EmbeddingRuntimeAvailability, HybridSearchConfig, HybridSearchHit, LlmSearchDoc,
    embedding_runtime_availability_from_env,
};

pub mod embedding {
    pub use crate::search::engine::EmbeddingRuntime;
}

pub mod hybrid {
    pub use crate::search::engine::{HybridSearchConfig, HybridSearchHit, LlmSearchDoc};
}

pub mod lexical {
    pub use crate::search::engine::LlmSearchDoc;
}

pub mod model_config {
    pub use crate::search::engine::{
        DEFAULT_BUNDLED_EMBED_MODEL_PATH, EMBEDDING_MAX_TOKENS_ENV, EMBEDDING_MODEL_ENV,
        EMBEDDING_MODEL_ID_ENV, EMBEDDING_RUNTIME_MODE_ENV, EMBEDDING_TOKENIZER_ENV,
    };
}

pub mod semantic {
    pub use crate::search::engine::HybridSearchHit;
}

pub mod tantivy_index {
    pub const ENGINE_IS_RUNTIME_OWNED: &str =
        "tantivy indexing stays behind SearchService in codestory-runtime";
}
