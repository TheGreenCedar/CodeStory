use anyhow::{Result, bail};
use std::path::Path;

pub fn refuse_existing_output(path: &Path) -> Result<()> {
    if path.exists() {
        bail!("proof_availability_output_exists");
    }
    Ok(())
}
