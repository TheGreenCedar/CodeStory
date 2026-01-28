//! Command Pattern Implementation for Undo/Redo
//!
//! Provides a command-based architecture for reversible operations.
//! This is a planned feature - commands are not yet integrated into the main application.
#![allow(dead_code)]

use codestory_core::NodeId;
use codestory_events::{Event, EventBus};
use std::any::Any;
use std::fmt::Debug;

/// Application state that commands can modify
pub struct AppState<'a> {
    pub active_node: &'a mut Option<NodeId>,
}

/// Trait for commands that can be executed and undone
pub trait Command: Debug + Send {
    /// Execute the command
    fn execute(&mut self, state: &mut AppState) -> Result<(), String>;

    /// Undo the command
    fn undo(&mut self, state: &mut AppState) -> Result<(), String>;

    /// Human-readable description
    fn description(&self) -> String;

    /// Support for downcasting
    fn as_any(&self) -> &dyn Any;

    /// Check if this command can be merged with another
    fn can_merge(&self, _other: &dyn Command) -> bool {
        false
    }

    /// Merge another command into this one (if can_merge returns true)
    fn merge(&mut self, _other: Box<dyn Command>) {}
}

/// Manages command history for undo/redo
pub struct CommandHistory {
    undo_stack: Vec<Box<dyn Command>>,
    redo_stack: Vec<Box<dyn Command>>,
    max_size: usize,
    event_bus: EventBus,
}

impl CommandHistory {
    pub fn new(max_size: usize, event_bus: EventBus) -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_size,
            event_bus,
        }
    }

    /// Execute a command and add it to the history
    pub fn execute(
        &mut self,
        mut cmd: Box<dyn Command>,
        state: &mut AppState,
    ) -> Result<(), String> {
        cmd.execute(state)?;

        // Try to merge with the last command
        if let Some(last) = self.undo_stack.last_mut()
            && last.can_merge(cmd.as_ref())
        {
            last.merge(cmd);
            self.notify_change();
            return Ok(());
        }

        // Clear redo stack on new command
        self.redo_stack.clear();

        // Add to undo stack
        self.undo_stack.push(cmd);

        // Enforce max size
        while self.undo_stack.len() > self.max_size {
            self.undo_stack.remove(0);
        }

        self.notify_change();
        Ok(())
    }

    /// Undo the last command
    pub fn undo(&mut self, state: &mut AppState) -> Result<(), String> {
        if let Some(mut cmd) = self.undo_stack.pop() {
            cmd.undo(state)?;
            self.redo_stack.push(cmd);
            self.notify_change();
            Ok(())
        } else {
            Err("Nothing to undo".to_string())
        }
    }

    /// Redo the last undone command
    pub fn redo(&mut self, state: &mut AppState) -> Result<(), String> {
        if let Some(mut cmd) = self.redo_stack.pop() {
            cmd.execute(state)?;
            self.undo_stack.push(cmd);
            self.notify_change();
            Ok(())
        } else {
            Err("Nothing to redo".to_string())
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo_description(&self) -> Option<String> {
        self.undo_stack.last().map(|c| c.description())
    }

    pub fn redo_description(&self) -> Option<String> {
        self.redo_stack.last().map(|c| c.description())
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.notify_change();
    }

    fn notify_change(&self) {
        self.event_bus.publish(Event::UndoStackChanged {
            can_undo: self.can_undo(),
            can_redo: self.can_redo(),
            undo_description: self.undo_description(),
            redo_description: self.redo_description(),
        });
    }
}

// ============================================================================
// Concrete Command Implementations
// ============================================================================

/// Command to activate/select a node
#[derive(Debug)]
pub struct ActivateNodeCommand {
    pub new_node_id: NodeId,
    pub previous_node_id: Option<NodeId>,
}

impl ActivateNodeCommand {
    pub fn new(new_node_id: NodeId) -> Self {
        Self {
            new_node_id,
            previous_node_id: None,
        }
    }
}

impl Command for ActivateNodeCommand {
    fn execute(&mut self, state: &mut AppState) -> Result<(), String> {
        self.previous_node_id = *state.active_node;
        *state.active_node = Some(self.new_node_id);
        Ok(())
    }

    fn undo(&mut self, state: &mut AppState) -> Result<(), String> {
        *state.active_node = self.previous_node_id;
        Ok(())
    }

    fn description(&self) -> String {
        format!("Select node {}", self.new_node_id.0)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// Helper trait for downcasting
trait CommandDowncast {
    fn downcast<T: 'static>(self: Box<Self>) -> Result<Box<T>, Box<dyn Command>>;
}

impl CommandDowncast for dyn Command {
    fn downcast<T: 'static>(self: Box<Self>) -> Result<Box<T>, Box<dyn Command>> {
        // This is a simplified implementation; in production use `downcast-rs` crate
        Err(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_activate_node_command() {
        let mut selected = None;
        let mut state = AppState {
            active_node: &mut selected,
        };

        let mut cmd = ActivateNodeCommand::new(NodeId(42));
        cmd.execute(&mut state).unwrap();
        assert_eq!(*state.active_node, Some(NodeId(42)));

        cmd.undo(&mut state).unwrap();
        assert_eq!(*state.active_node, None);
    }
}
