//! A code editor for files opened from the terminal, with help from language servers.

pub mod language;
pub mod lsp;
pub mod pane;
pub mod sudo;

pub use pane::{EditorPane, EditorPaneEvent};
