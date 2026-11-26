use crate::*;

// ---- The page's API -------------------------------------------------------

#[tauri::command]
pub fn store_get(key: String, store: State<Store>) -> Value {
    store
        .map
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .cloned()
        .unwrap_or(Value::Null)
}

#[tauri::command]
pub fn store_set(key: String, value: Value, store: State<Store>) -> Result<bool, String> {
    let mut map = store
        .map
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.insert(key, value);
    if !store.persist {
        return Ok(true);
    }
    let bytes = serde_json::to_vec(&*map).map_err(|e| e.to_string())?;
    // Write then rename, so a crash never leaves half a file.
    let tmp = store.file.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &store.file).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
pub fn notify(text: String, app: AppHandle, me: State<App>) -> Result<bool, String> {
    if me.check.is_some() || me.making {
        return Ok(true);
    }
    if powers::state(&me.folder, "notifications") != powers::Grant::Yes {
        return Err("not-allowed: call app.permissions.request(['notifications']) first".into());
    }
    app.notification()
        .builder()
        .title(&me.name)
        .body(text)
        .show()
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// Only the load check may call this; for an ordinary app window it does nothing.
#[tauri::command]
pub fn check_report(mut report: Value, app: AppHandle, me: State<App>, store: State<Store>) {
    if let Some(path) = &me.check {
        // What the app saved as it opened (kept in memory; nothing is written).
        report["store_keys"] = json!(store
            .map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect::<Vec<_>>());
        let _ = std::fs::write(path, report.to_string());
        app.exit(0);
    }
}
