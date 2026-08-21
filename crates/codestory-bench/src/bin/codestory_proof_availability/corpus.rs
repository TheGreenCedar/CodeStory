use super::contracts::{CorpusV1, ThresholdsV1};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub fn load(path: &Path) -> Result<CorpusV1> {
    let bytes = fs::read(path).with_context(|| format!("read corpus {}", path.display()))?;
    CorpusV1::from_json(
        serde_json::from_slice(&bytes).context("parse proof availability corpus JSON")?,
    )
}

pub fn load_thresholds(path: &Path) -> Result<ThresholdsV1> {
    let bytes = fs::read(path).with_context(|| format!("read thresholds {}", path.display()))?;
    ThresholdsV1::from_json(
        serde_json::from_slice(&bytes).context("parse proof availability thresholds JSON")?,
    )
}
