use codestory_core::NodeKind;
use parking_lot::RwLock;
use std::collections::HashMap;

/// A thread-safe symbol table for tracking node kinds during indexing.
/// This helps reduce the creation of UNKNOWN nodes for symbols defined in other files.
pub struct SymbolTable {
    symbols: RwLock<HashMap<i64, NodeKind>>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            symbols: RwLock::new(HashMap::new()),
        }
    }

    /// Insert or update a symbol's kind.
    /// Concrete kinds (like FUNCTION, CLASS) always overwrite UNKNOWN.
    pub fn insert(&self, id: i64, kind: NodeKind) {
        let mut symbols = self.symbols.write();
        if let Some(existing) = symbols.get_mut(&id) {
            if *existing == NodeKind::UNKNOWN && kind != NodeKind::UNKNOWN {
                *existing = kind;
            }
        } else {
            symbols.insert(id, kind);
        }
    }

    pub fn get(&self, id: i64) -> Option<NodeKind> {
        self.symbols.read().get(&id).cloned()
    }

    pub fn len(&self) -> usize {
        self.symbols.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.read().is_empty()
    }
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_get() {
        let table = SymbolTable::new();

        table.insert(1, NodeKind::FUNCTION);
        table.insert(2, NodeKind::CLASS);

        assert_eq!(table.get(1), Some(NodeKind::FUNCTION));
        assert_eq!(table.get(2), Some(NodeKind::CLASS));
        assert_eq!(table.get(3), None);
    }

    #[test]
    fn test_unknown_upgrade() {
        let table = SymbolTable::new();

        // Insert UNKNOWN first
        table.insert(1, NodeKind::UNKNOWN);
        assert_eq!(table.get(1), Some(NodeKind::UNKNOWN));

        // Upgrade to concrete type
        table.insert(1, NodeKind::FUNCTION);
        assert_eq!(table.get(1), Some(NodeKind::FUNCTION));

        // Verify it stays FUNCTION
        table.insert(1, NodeKind::UNKNOWN);
        assert_eq!(table.get(1), Some(NodeKind::FUNCTION));
    }

    #[test]
    fn test_concrete_type_not_overwritten() {
        let table = SymbolTable::new();

        table.insert(1, NodeKind::FUNCTION);
        assert_eq!(table.get(1), Some(NodeKind::FUNCTION));

        // Try to insert different concrete type - should not change
        table.insert(1, NodeKind::CLASS);
        assert_eq!(table.get(1), Some(NodeKind::FUNCTION));
    }

    #[test]
    fn test_len_and_empty() {
        let table = SymbolTable::new();

        assert!(table.is_empty());
        assert_eq!(table.len(), 0);

        table.insert(1, NodeKind::FUNCTION);
        assert!(!table.is_empty());
        assert_eq!(table.len(), 1);

        table.insert(2, NodeKind::CLASS);
        assert_eq!(table.len(), 2);
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let table = Arc::new(SymbolTable::new());
        let mut handles = vec![];

        // Spawn multiple threads inserting different symbols
        for i in 0..10 {
            let table_clone = Arc::clone(&table);
            let handle = thread::spawn(move || {
                for j in 0..100 {
                    let id = (i * 100 + j) as i64;
                    table_clone.insert(id, NodeKind::FUNCTION);
                }
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Verify all symbols were inserted
        assert_eq!(table.len(), 1000);
        assert_eq!(table.get(500), Some(NodeKind::FUNCTION));
    }
}
