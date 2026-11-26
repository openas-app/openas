use super::*;

// ---- Spend ------------------------------------------------------------------

/// What making one app costs, before anything is spent: the token profile we
/// measured on real builds (low, typical, high; changes are about the same),
/// times this model's price. Claude reuses most of its input from cache; an
/// OpenAI-compatible endpoint is counted at full price. None if the price
/// isn't known.
pub fn estimate(provider: &str, model: &str) -> Option<Value> {
    const PROFILE: [(f64, f64); 3] = [(0.6e6, 40e3), (1.5e6, 55e3), (2.6e6, 100e3)];
    let p = crate::model::providers().ok()?.get(provider).cloned()?;
    let each: Vec<f64> = if model.starts_with("claude-") {
        if !matches!(
            model,
            "claude-opus-5-5" | "claude-sonnet-5-5" | "claude-haiku-5-5"
        ) {
            return None;
        }
        PROFILE
            .iter()
            .map(|&(i, o)| {
                let u = json!({ "input_tokens": (i * 0.02) as u64, "cache_creation_input_tokens": (i * 0.08) as u64,
                                "cache_read_input_tokens": (i * 0.90) as u64, "output_tokens": o as u64 });
                claude::cost(model, &u)
            })
            .collect()
    } else {
        let (pi, po) = crate::oai::known_price(&p.base_url, model)?;
        PROFILE
            .iter()
            .map(|&(i, o)| i / 1e6 * pi + o / 1e6 * po)
            .collect()
    };
    Some(json!({ "low": each[0], "typical": each[1], "high": each[2] }))
}

/// One ledger for every run on this Mac, whatever OPENAS_ROOT says, so a
/// scratch root can't start the count again at zero.
pub(super) fn ledger_dir() -> PathBuf {
    if let Ok(d) = std::env::var("OPENAS_LEDGER_DIR") {
        return PathBuf::from(d);
    }
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let dir = home.join("Library/Application Support/OpenAs");
    // The ledger from before the rename comes along once, so the count goes on.
    let old = home.join("Library/Application Support/Open Apps POC");
    if !dir.join("spend.json").exists() && old.join("spend.json").exists() {
        let _ = std::fs::create_dir_all(&dir);
        for f in ["spend.json", "spend.jsonl"] {
            let _ = std::fs::copy(old.join(f), dir.join(f));
        }
    }
    dir
}

pub(super) fn ledger_path() -> PathBuf {
    ledger_dir().join("spend.json")
}

/// "2026-11" for a Unix time, in UTC (civil-from-days, Howard Hinnant).
pub(super) fn month_of(t: u64) -> String {
    let z = (t / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y}-{m:02}")
}

/// What was spent this calendar month, from the per-step log. A log that
/// exists but can't be read counts as over any limit.
pub fn spent_this_month() -> f64 {
    let Ok(text) = std::fs::read_to_string(ledger_dir().join("spend.jsonl")) else {
        return 0.0;
    };
    let month = month_of(now());
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["t"].as_u64().is_some_and(|t| month_of(t) == month))
        .filter_map(|v| v["usd"].as_f64())
        .sum()
}

/// The limits in force: the person's settings, unless the environment
/// overrides them (tests and comparisons do).
pub fn limits() -> (f64, f64) {
    let s = crate::model::settings();
    (
        env_f("OPENAS_SPEND_CAP", s.monthly_limit),
        env_f("OPENAS_APP_CAP", s.app_limit),
    )
}

/// The total so far. A ledger that exists but can't be read stops spending
/// (counts as over any cap) rather than starting again at zero.
pub fn spent() -> (f64, Value) {
    match std::fs::read_to_string(ledger_path()) {
        Err(_) => (0.0, json!({})),
        Ok(t) => match serde_json::from_str::<Value>(&t) {
            Ok(v) => (
                v.get("total")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::INFINITY),
                v,
            ),
            Err(_) => (f64::INFINITY, json!({ "unreadable": true })),
        },
    }
}

pub(super) fn add_spend(job: &str, dollars: f64, usage: &Value) -> Result<f64> {
    // One writer at a time across processes (two builds can run at once).
    std::fs::create_dir_all(ledger_dir())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(ledger_dir().join("spend.lock"))?;
    // Released when `lock` is dropped at the end of this function.
    lock.lock()?;
    let (total, mut v) = spent();
    if v.get("unreadable").is_some() {
        bail!("the spend ledger can't be read; stopping rather than counting from zero");
    }
    let total = total + dollars;
    v["total"] = json!(total);
    let j = v["jobs"][job].as_f64().unwrap_or(0.0) + dollars;
    v["jobs"][job] = json!(j);
    let tmp = ledger_dir().join("spend.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&v)?)?;
    std::fs::rename(&tmp, ledger_path())?;
    let line = json!({ "t": now(), "job": job, "model": model(), "usd": dollars, "usage": usage });
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ledger_dir().join("spend.jsonl"))?;
    writeln!(f, "{line}")?;
    Ok(j)
}

#[cfg(test)]
mod month_tests {
    use super::month_of;

    #[test]
    fn months_fall_on_calendar_boundaries() {
        assert_eq!(month_of(0), "1970-01");
        assert_eq!(month_of(1_709_164_800), "2024-02"); // 29 Feb 2024
        assert_eq!(month_of(1_709_251_200), "2024-03"); // 1 Mar 2024, 00:00 UTC
        assert_eq!(month_of(1_798_675_200 - 1), "2026-12");
    }
}
