use serde::{Deserialize, Serialize};

/// Capability flags derived from probes (not just HTTP reachability).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SidecarCapabilities {
    pub lexical: bool,
    pub semantic: bool,
    pub graph: bool,
}

impl SidecarCapabilities {
    pub const NONE: Self = Self {
        lexical: false,
        semantic: false,
        graph: false,
    };

    pub fn production_stack() -> Self {
        Self {
            lexical: true,
            semantic: true,
            graph: true,
        }
    }
}
