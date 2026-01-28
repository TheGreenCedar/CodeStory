use codestory_core::NodeId;
use codestory_events::ActivationOrigin;
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigationEntry {
    pub node_ids: Vec<NodeId>,
    #[serde(skip, default = "Instant::now")]
    pub timestamp: Instant,
    pub origin: ActivationOrigin,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct NavigationHistory {
    entries: Vec<NavigationEntry>,
    current: usize,
}

impl NavigationHistory {
    pub fn push(&mut self, entry: NavigationEntry) {
        // If we are not at the end of history, truncate it
        if !self.entries.is_empty() && self.current < self.entries.len() - 1 {
            self.entries.truncate(self.current + 1);
        }

        // Don't push duplicate consecutive entries
        if let Some(last) = self.entries.last()
            && last.node_ids == entry.node_ids
        {
            return;
        }

        self.entries.push(entry);
        self.current = self.entries.len() - 1;
    }

    pub fn back(&mut self) -> Option<&NavigationEntry> {
        if self.current > 0 {
            self.current -= 1;
            Some(&self.entries[self.current])
        } else {
            None
        }
    }

    pub fn forward(&mut self) -> Option<&NavigationEntry> {
        if self.current + 1 < self.entries.len() {
            self.current += 1;
            Some(&self.entries[self.current])
        } else {
            None
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.current > 0
    }

    pub fn can_go_forward(&self) -> bool {
        !self.entries.is_empty() && self.current < self.entries.len() - 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TabId(pub uuid::Uuid);

impl TabId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub active_node: Option<NodeId>,
    pub history: NavigationHistory,
    // Add scroll states etc later
}

impl Tab {
    pub fn new(title: String) -> Self {
        Self {
            id: TabId::new(),
            title,
            active_node: None,
            history: NavigationHistory::default(),
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TabManager {
    pub tabs: Vec<Tab>,
    pub active_tab_index: usize,
}

impl TabManager {
    pub fn new() -> Self {
        Self {
            tabs: vec![Tab::new("Welcome".to_string())],
            active_tab_index: 0,
        }
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab_index)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active_tab_index)
    }

    pub fn open_tab(&mut self, title: String, node_id: Option<NodeId>) -> TabId {
        let mut tab = Tab::new(title);
        tab.active_node = node_id;
        let id = tab.id;
        self.tabs.push(tab);
        self.active_tab_index = self.tabs.len() - 1;
        id
    }

    pub fn close_tab(&mut self, index: usize) {
        if self.tabs.len() <= 1 {
            return; // Keep at least one tab
        }
        self.tabs.remove(index);
        if self.active_tab_index >= self.tabs.len() {
            self.active_tab_index = self.tabs.len() - 1;
        }
    }

    pub fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab_index = index;
        }
    }
}
