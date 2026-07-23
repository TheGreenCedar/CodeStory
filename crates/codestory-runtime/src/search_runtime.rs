pub(crate) use crate::search::engine::SearchEngine;
pub use crate::search::engine::{
    EmbeddingProfileContract, EmbeddingRuntimeAvailability, HybridSearchConfig, HybridSearchHit,
    LlmSearchDoc, STORED_VECTOR_ENCODING_ENV, embedding_profile_contract_from_config,
    embedding_profile_contract_from_env, embedding_runtime_availability_from_config,
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
    pub use crate::search::engine::STORED_VECTOR_ENCODING_ENV;
}

pub mod semantic {
    pub use crate::search::engine::HybridSearchHit;
}

pub mod tantivy_index {
    pub const ENGINE_IS_RUNTIME_OWNED: &str =
        "tantivy indexing stays behind SearchService in codestory-runtime";
}
