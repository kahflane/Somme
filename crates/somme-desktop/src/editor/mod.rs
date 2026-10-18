//! A code editor for files opened from the terminal or in their own tabs, with
//! help from language servers.

pub mod big;
pub mod blame;
pub mod jq;
pub mod language;
pub mod lsp;
pub mod markdown;
pub mod pane;
pub mod scratch;
pub mod servers;
pub mod sudo;
pub mod system_open;
pub mod tools;

pub use pane::{EditorPane, EditorPaneEvent};
