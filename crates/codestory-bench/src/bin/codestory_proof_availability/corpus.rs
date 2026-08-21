use super::contracts::{CohortPathFileV1, CorpusV1, ThresholdsV1};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub fn load(path: &Path) -> Result<CorpusV1> {
    let bytes = fs::read(path).with_context(|| format!("read corpus {}", path.display()))?;
    CorpusV1::from_json(
        serde_json::from_slice(&bytes).context("parse proof availability corpus JSON")?,
    )
}

pub struct LoadedCorpusV1 {
    pub corpus: CorpusV1,
    pub path_files: Vec<CohortPathFileV1>,
}

pub fn load_complete(path: &Path) -> Result<LoadedCorpusV1> {
    let corpus = load(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("proof_availability_corpus_parent_missing"))?;
    let mut path_files = Vec::with_capacity(corpus.cohorts.len());
    for cohort in &corpus.cohorts {
        let path_file_path = parent.join(&cohort.path_file);
        let bytes = fs::read(&path_file_path)
            .with_context(|| format!("read cohort path file {}", path_file_path.display()))?;
        path_files.push(
            CohortPathFileV1::from_json(
                serde_json::from_slice(&bytes).context("parse cohort path file JSON")?,
            )
            .with_context(|| format!("validate cohort path file {}", path_file_path.display()))?,
        );
    }
    corpus.validate_with_path_files(&path_files)?;
    Ok(LoadedCorpusV1 { corpus, path_files })
}

pub fn load_thresholds(path: &Path) -> Result<ThresholdsV1> {
    let bytes = fs::read(path).with_context(|| format!("read thresholds {}", path.display()))?;
    ThresholdsV1::from_json(
        serde_json::from_slice(&bytes).context("parse proof availability thresholds JSON")?,
    )
}
