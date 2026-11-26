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
