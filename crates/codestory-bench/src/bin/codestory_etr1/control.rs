//! One deadline and cancellation source for the complete paired experiment.
use anyhow::{Result, ensure};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const PAIRED_RUN_LIMIT: Duration = Duration::from_secs(30 * 60);

pub struct RunControl {
    started: Instant,
    cancel_file: PathBuf,
    limit: Duration,
}

impl RunControl {
    pub fn new(cancel_file: &Path) -> Result<Self> {
        ensure!(cancel_file.is_absolute(), "cancel_file_must_be_absolute");
        let control = Self {
            started: Instant::now(),
            cancel_file: cancel_file.into(),
            limit: PAIRED_RUN_LIMIT,
        };
        control.check()?;
        Ok(control)
    }

    pub fn cancelled(&self) -> bool {
        self.cancel_file.try_exists().unwrap_or(true) || self.started.elapsed() >= self.limit
    }

    pub fn check(&self) -> Result<()> {
        ensure!(!self.cancel_file.try_exists()?, "etr1_cancelled");
        ensure!(
            self.started.elapsed() < self.limit,
            "etr1_deadline_exceeded"
        );
        Ok(())
    }

    pub fn batch_timeout(&self) -> Result<Duration> {
        self.check()?;
        Ok(self
            .limit
            .saturating_sub(self.started.elapsed())
            .min(Duration::from_secs(60)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_and_global_deadline_stop_every_later_batch() {
        let root = tempfile::tempdir().unwrap();
        let cancel_file = root.path().join("cancel");
        let mut control = RunControl::new(&cancel_file).unwrap();
        assert!(!control.cancelled());
        std::fs::write(&cancel_file, b"cancel").unwrap();
        assert!(control.cancelled());
        assert!(
            control
                .batch_timeout()
                .unwrap_err()
                .to_string()
                .contains("etr1_cancelled")
        );
        std::fs::remove_file(&cancel_file).unwrap();
        control.limit = Duration::ZERO;
        assert!(control.cancelled());
        assert!(
            control
                .batch_timeout()
                .unwrap_err()
                .to_string()
                .contains("etr1_deadline_exceeded")
        );
    }
}
