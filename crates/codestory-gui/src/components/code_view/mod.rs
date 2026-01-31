pub mod enhanced;
pub mod multi_file;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeViewMode {
    SingleFile,
    Snippets,
}

#[allow(unused_imports)]
pub use multi_file::{ClickAction, FileSnippet, MultiFileCodeView};
