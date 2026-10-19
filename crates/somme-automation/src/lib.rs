//! Automations: flows of steps that run commands on saved servers and on
//! this computer, check results and tell the user. No GPUI here, so the
//! app, the background worker and later runners all use the same engine.

pub mod agent;
pub mod computer;
pub mod dry;
pub mod engine;
pub mod history;
pub mod layout;
pub mod net;
pub mod nodes;
pub mod schedule;
pub mod schema;
pub mod templates;
pub mod values;

pub use engine::{Event, RunStatus, run};
pub use history::{History, RunHead, RunRecord, StartedBy};
pub use nodes::{HostRef, Output, Tools};
pub use schema::{Automation, Edge, Node, NodeKind};
