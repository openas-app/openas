//! The builder that works in steps: a model with a few file tools and a
//! `check` that shows it the app (screenshot, errors, a test script), in a
//! loop until it calls `finish`. The tools are plain JSON schemas so the same
//! loop can later drive any model with function calling. Today it talks to
//! Claude through the Messages API.
//!
//! Spend is enforced here: every turn's cost is added to `<root>/spend.json`
//! and checked before the next turn starts.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::claude::{self, Claude, Ev};
use crate::{check, keys, make, paths};

pub(super) const BUILDER: &str = include_str!("../../prompts/builder.md");
pub const MODEL_DEFAULT: &str = "claude-opus-5-5";
pub(super) fn model() -> String {
    std::env::var("OPENAS_AGENT_MODEL").unwrap_or_else(|_| crate::model::settings().build_model)
}
/// The last backstop. A build normally ends long before: it finishes, or
/// the progress checks below see it going in circles.
/// Pulls the "doing" line out of a tool call while it is still streaming.
pub(super) static DOING_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#""doing"\s*:\s*"((?:[^"\\]|\\.)*)""#).expect("valid regex")
});
pub(super) const MAX_TURNS: usize = 200;
/// The same failing tool call this many times in a row: a nudge, then stop.
pub(super) const SAME_FAIL_NUDGE: usize = 3;
pub(super) const SAME_FAIL_STOP: usize = 6;
/// Steps in a row without a file changing: a nudge, then stop.
pub(super) const IDLE_NUDGE: usize = 20;
pub(super) const IDLE_STOP: usize = 32;
/// Steps before the end when the builder is told to wrap up: open models
/// work in small steps and keep polishing until the limit cuts them off.
pub(super) const WRAP_UP: usize = 15;

pub(super) fn env_f(k: &str, d: f64) -> f64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub struct Finished {
    pub name: String,
    pub summary: String,
    /// Set when the builder stopped before calling finish but left an app
    /// that opens: why it stopped, said to the person.
    pub unfinished: Option<String>,
}

mod draft;
mod live;
mod spend;
mod tools;
mod turn;

pub use draft::{change, make_run, new_draft, prepare_draft, start_make};
pub use live::Live;
pub use spend::{estimate, spent};
pub use turn::run;

// The modules share their helpers with each other through here.
use spend::*;
use tools::*;
