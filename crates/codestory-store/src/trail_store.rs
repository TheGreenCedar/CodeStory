use crate::{StorageError, Store};
use codestory_contracts::trail::{TrailConfig, TrailResult};

pub struct TrailStore<'a> {
    storage: &'a Store,
}

impl<'a> TrailStore<'a> {
    pub(crate) fn new(storage: &'a Store) -> Self {
        Self { storage }
    }

    pub fn get_trail(&self, config: &TrailConfig) -> Result<TrailResult, StorageError> {
        self.storage.get_trail(config)
    }
}
