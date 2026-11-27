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

/// Pulls the "doing" line out of a tool call while it is still streaming.
pub(super) static DOING_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#""doing"\s*:\s*"((?:[^"\\]|\\.)*)""#).expect("valid regex")
});
/// Starts a spending-limit error, so the column can offer a way out.
pub const LIMIT_MARK: &str = "[limit]";

const BUILDER_MD: &str = include_str!("../../prompts/builder.md");

/// Where the builder's instructions name the Mac, what they say on Windows.
const ON_WINDOWS: [(&str, &str); 12] = [
    ("on their own Mac", "on their own Windows PC"),
    ("its own name and icon in the Dock", "its own name and icon on the taskbar"),
    ("macOS asks the person the first time", "Windows asks the person the first time"),
    ("(shows it in Finder)", "(shows it in File Explorer)"),
    ("full access to the Mac", "full access to the computer"),
    ("in the macOS menu bar (text only, no image; a single symbol character like ◷ may lead the title)", "in the system tray (Windows shows the app's icon there, with the title as its tooltip)"),
    ("While it's there, closing the window keeps the app running in the menu bar", "While it's there, closing the window keeps the app running in the tray"),
    ("\"/bin/zsh\" with [\"-lc\", \"…\"]", "\"cmd\" with [\"/c\", \"…\"] or \"powershell\" with [\"-NoProfile\", \"-Command\", \"…\"]"),
    ("everyday ones like ⌘C", "everyday ones like Ctrl+C"),
    ("keyboard shortcuts a Mac user expects", "keyboard shortcuts a Windows user expects"),
    ("Feels like a Mac app", "Feels like a Windows app"),
    ("OpenAs gives it the macOS icon shape, size and shadow", "OpenAs gives it a rounded shape, size and shadow"),
];

/// The builder's instructions for the system it runs on.
pub(super) fn builder_prompt() -> std::borrow::Cow<'static, str> {
    if cfg!(windows) {
        let mut s = BUILDER_MD.to_string();
        for (mac, win) in ON_WINDOWS {
            s = s.replace(mac, win);
        }
        std::borrow::Cow::Owned(s)
    } else {
        std::borrow::Cow::Borrowed(BUILDER_MD)
    }
}
pub const MODEL_DEFAULT: &str = "claude-opus-5-5";
pub(super) fn model() -> String {
    std::env::var("OPENAS_AGENT_MODEL").unwrap_or_else(|_| crate::model::settings().build_model)
}
/// The last backstop. A build normally ends long before: it finishes, or
/// the progress checks below see it going in circles.
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

pub use draft::{
    change, discard_change, make_run, new_draft, pending_dir, prepare_draft, save_change,
    start_make,
};
pub use live::Live;
pub use spend::{estimate, limits, spent, spent_this_month};
pub use turn::run;

// The modules share their helpers with each other through here.
use spend::*;
use tools::*;

#[cfg(test)]
mod prompt_tests {
    /// Every Mac phrase the Windows wording replaces is still in the
    /// instructions (an edit there must update the list).
    #[test]
    fn the_windows_wording_still_applies() {
        for (mac, _) in super::ON_WINDOWS {
            assert!(
                super::BUILDER_MD.contains(mac),
                "builder.md no longer says: {mac}"
            );
        }
    }
}
