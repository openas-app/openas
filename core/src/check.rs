//! The load check: open the app in the Runner with no window shown, let it
//! run for a moment, and report what went wrong. It catches broken and empty
//! apps; it doesn't click through them.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::paths;

#[derive(Debug, Default, Deserialize)]
pub struct Report {
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub dialogs: Vec<String>,
    #[serde(default)]
    pub text_chars: u64,
    #[serde(default)]
    pub elements: u64,
    #[serde(default)]
    pub controls: u64,
    #[serde(default)]
    pub canvases: u64,
    #[serde(default)]
    pub timeout: bool,
    /// The keys in the app's data after it opened (what it saved on load).
    #[serde(default)]
    pub store_keys: Vec<String>,
}

impl Report {
    pub fn problems(&self) -> Vec<String> {
        let mut out = vec![];
        if self.timeout {
            out.push("The app never finished loading.".into());
        }
        for e in self.errors.iter().take(5) {
            out.push(format!("Error while loading: {e}"));
        }
        for d in &self.dialogs {
            out.push(format!("It called {d}(); use in-page UI instead."));
        }
        if self.text_chars < 20 && self.canvases == 0 {
            out.push("The window is empty: no visible text after loading. Show the app with sample data.".into());
        }
        out
    }
}

/// The hosts a page tried to reach (blocked, since apps start offline).
pub fn wanted_hosts(problems: &[String]) -> Vec<String> {
    let mut hosts: Vec<String> = problems
        .iter()
        .filter_map(|p| p.split("It tried to reach ").nth(1))
        .filter_map(|rest| rest.split("://").nth(1))
        .filter_map(|rest| rest.split(['/', '?', ':', ' ']).next())
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

pub fn run(folder: &Path) -> Result<Report> {
    let report = folder.join(".check.json");
    let _ = std::fs::remove_file(&report);
    let mut child = Command::new(paths::runner_exe())
        .arg("--check")
        .arg(folder)
        .arg("--report")
        .arg(&report)
        // Never onto our stdout: the MCP server speaks JSON-RPC there.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("starting the Runner for the load check")?;
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            return Ok(Report {
                timeout: true,
                ..Default::default()
            });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let text = std::fs::read_to_string(&report).unwrap_or_default();
    let _ = std::fs::remove_file(&report);
    let r: Report = serde_json::from_str(&text).unwrap_or(Report {
        timeout: true,
        ..Default::default()
    });
    let _ = std::fs::write(
        folder.join(".store-keys.json"),
        serde_json::to_string(&r.store_keys).unwrap_or_default(),
    );
    Ok(r)
}

#[cfg(test)]
mod tests {
    #[test]
    fn hosts_come_from_blocked_requests_only() {
        let p = vec![
            "Error while loading: It tried to reach https://api.open-meteo.com/v1/forecast?x=1. Apps have no internet access".to_string(),
            "Error while loading: It tried to reach wss://feed.example.org:443/live. Apps have no internet access".to_string(),
            "Error while loading: Unhandled: Load failed".to_string(),
        ];
        assert_eq!(
            super::wanted_hosts(&p),
            vec!["api.open-meteo.com", "feed.example.org"]
        );
        assert!(super::wanted_hosts(&["It called confirm()".to_string()]).is_empty());
    }
}
