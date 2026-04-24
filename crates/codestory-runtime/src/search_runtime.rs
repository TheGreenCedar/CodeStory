pub(crate) use crate::search::engine::SearchEngine;
pub use crate::search::engine::{
    EMBEDDING_BACKEND_ENV, EMBEDDING_DOCUMENT_PREFIX_ENV, EMBEDDING_EXPECTED_DIM_ENV,
    EMBEDDING_LAYER_NORM_ENV, EMBEDDING_MAX_TOKENS_ENV, EMBEDDING_MODEL_ID_ENV,
    EMBEDDING_POOLING_ENV, EMBEDDING_PROFILE_ENV, EMBEDDING_QUERY_PREFIX_ENV,
    EMBEDDING_RUNTIME_MODE_ENV, EMBEDDING_TRUNCATE_DIM_ENV, EmbeddingRuntimeAvailability,
    HybridSearchConfig, HybridSearchHit, LLAMACPP_EMBEDDINGS_URL_ENV, LLAMACPP_REQUEST_COUNT_ENV,
    LlmSearchDoc, STORED_VECTOR_ENCODING_ENV, embedding_runtime_availability_from_env,
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
        EMBEDDING_BACKEND_ENV, EMBEDDING_DOCUMENT_PREFIX_ENV, EMBEDDING_EXPECTED_DIM_ENV,
        EMBEDDING_LAYER_NORM_ENV, EMBEDDING_MAX_TOKENS_ENV, EMBEDDING_MODEL_ID_ENV,
        EMBEDDING_POOLING_ENV, EMBEDDING_PROFILE_ENV, EMBEDDING_QUERY_PREFIX_ENV,
        EMBEDDING_RUNTIME_MODE_ENV, EMBEDDING_TRUNCATE_DIM_ENV, LLAMACPP_EMBEDDINGS_URL_ENV,
        LLAMACPP_REQUEST_COUNT_ENV, STORED_VECTOR_ENCODING_ENV,
    };
}

pub mod semantic {
    pub use crate::search::engine::HybridSearchHit;
}

pub mod tantivy_index {
    pub const ENGINE_IS_RUNTIME_OWNED: &str =
        "tantivy indexing stays behind SearchService in codestory-runtime";
}
