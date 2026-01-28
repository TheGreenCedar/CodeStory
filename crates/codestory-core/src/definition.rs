use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum DefinitionKind {
    #[default]
    Explicit,
    Implicit,
}
