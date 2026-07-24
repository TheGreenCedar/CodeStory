//! Authenticated product-process worker for embedding qualification.

use crate::args::InternalEmbeddingQualificationCommand;
use anyhow::Result;

mod worker;

pub(crate) fn run_internal_embedding_qualification_worker(
    command: InternalEmbeddingQualificationCommand,
) -> Result<()> {
    worker::run(command)
}
