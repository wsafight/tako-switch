fn claude_settings_path() -> std::path::PathBuf {
    crate::config::get_home_dir()
        .join(".claude")
        .join("settings.json")
}

fn statusline_command() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "tako-switch".to_string());
    format!("\"{exe}\" statusline")
}

#[tauri::command]
pub async fn tako_statusline_status() -> Result<bool, String> {
    let path = claude_settings_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let cmd = json
        .get("statusLine")
        .and_then(|s| s.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    Ok(cmd.contains("statusline"))
}

#[tauri::command]
pub async fn tako_statusline_enable() -> Result<bool, String> {
    let path = claude_settings_path();
    let mut json: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    json["statusLine"] = serde_json::json!({
        "type": "command",
        "command": statusline_command(),
        "padding": 0,
    });

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir failed: {e}"))?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap())
        .map_err(|e| format!("write failed: {e}"))?;
    Ok(true)
}

#[tauri::command]
pub async fn tako_statusline_disable() -> Result<bool, String> {
    let path = claude_settings_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(true);
    };
    let mut json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let is_tako = json
        .get("statusLine")
        .and_then(|s| s.get("command"))
        .and_then(|c| c.as_str())
        .map(|c| c.contains("statusline"))
        .unwrap_or(false);
    if is_tako {
        if let Some(obj) = json.as_object_mut() {
            obj.remove("statusLine");
        }
        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap())
            .map_err(|e| format!("write failed: {e}"))?;
    }
    Ok(true)
}
