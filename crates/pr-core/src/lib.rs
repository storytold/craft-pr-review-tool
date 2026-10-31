//! Everything Craft PR Review knows that isn't network or UI: the PR model,
//! the CodeRabbit parser, the landing heuristics, overlap detection, the fetch
//! schedule, what's kept on disk and the views the UI reads.

pub mod assess;
pub mod coderabbit;
pub mod config;
pub mod model;
pub mod overlap;
pub mod schedule;
pub mod sibling;
pub mod store;
pub mod views;

pub use assess::{assess, author_info, ci_state, Assessment, Reason};
pub use config::{ClaudeConfig, Config};
pub use model::*;
pub use overlap::Group;
pub use schedule::{next_job, DetailRef, Job, SchedInput};
pub use sibling::Sibling;
pub use store::Store;

/// Shared with Craft Status.
pub use craft_core::{atomic_write, Error, Result};
