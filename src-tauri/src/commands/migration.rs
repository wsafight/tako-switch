#![allow(non_snake_case)]

//! First-run migration helpers. Tako Switch can import data from two sources,
//! always prompting the user first (never silent):
//!   1. ~/.cc-switch  — original cc-switch config + database (old users)
//!   2. ~/.tako       — Tako CLI config (apiKey / providers / login state)
//!
//! Detection commands return what's available; import commands do the copy
//! after the user confirms in the UI.

use serde::Serialize;
use std::fs;
use std::path::PathBuf;

fn home() -> PathBuf {
    crate::config::get_home_dir()
}

fn tako_switch_dir() -> PathBuf {
    crate::config::get_app_config_dir()
}

#[derive(Serialize, Default)]
pub struct MigrationDetect {
    /// ~/.cc-switch exists with real data and we haven't imported yet.
    pub ccswitch_available: bool,
    /// ~/.tako/config.json exists with a Tako apiKey.
    pub tako_cli_available: bool,
    /// The Tako apiId (not the key) for display, if detected.
    pub tako_account_id: Option<String>,
}

/// Detect what can be imported. Called on startup; the UI shows a prompt.
#[tauri::command]
pub async fn migration_detect() -> Result<MigrationDetect, String> {
    let mut out = MigrationDetect::default();
    let marker = tako_switch_dir().join(".migrated");

    // cc-switch: only offer if old dir has real data AND we haven't imported.
    let ccswitch_dir = home().join(".cc-switch");
    let has_ccswitch_data = ccswitch_dir.join("cc-switch.db").exists()
        || ccswitch_dir.join("config.json").exists();
    let already_imported_cc = marker_contains(&marker, "ccswitch");
    out.ccswitch_available = has_ccswitch_data && !already_imported_cc;

    // Tako CLI: offer if ~/.tako/config.json has an apiKey.
    let tako_config = home().join(".tako").join("config.json");
    if tako_config.exists() {
        if let Ok(text) = fs::read_to_string(&tako_config) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if json.get("apiKey").and_then(|v| v.as_str()).is_some() {
                    out.tako_cli_available = !marker_contains(&marker, "tako-cli");
                    out.tako_account_id = json
                        .get("apiId")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
            }
        }
    }
    Ok(out)
}

fn marker_contains(marker: &PathBuf, tag: &str) -> bool {
    fs::read_to_string(marker)
        .map(|s| s.lines().any(|l| l.trim() == tag))
        .unwrap_or(false)
}

fn mark_done(tag: &str) {
    let dir = tako_switch_dir();
    let _ = fs::create_dir_all(&dir);
    let marker = dir.join(".migrated");
    let mut content = fs::read_to_string(&marker).unwrap_or_default();
    if !content.lines().any(|l| l.trim() == tag) {
        content.push_str(tag);
        content.push('\n');
        let _ = fs::write(&marker, content);
    }
}

/// Copy ~/.cc-switch data into ~/.tako-switch (after user confirms).
#[tauri::command]
pub async fn migration_import_ccswitch() -> Result<bool, String> {
    let src = home().join(".cc-switch");
    let dst = tako_switch_dir();
    if !src.exists() {
        return Err("No ~/.cc-switch directory found".into());
    }
    fs::create_dir_all(&dst).map_err(|e| format!("Failed to create target dir: {e}"))?;

    // Copy config.json and the database, renaming the db to the Tako name.
    for (from_name, to_name) in [("config.json", "config.json"), ("cc-switch.db", "tako-switch.db")] {
        let from = src.join(from_name);
        if from.exists() {
            let to = dst.join(to_name);
            // Don't clobber an existing Tako file the user already created.
            if !to.exists() {
                fs::copy(&from, &to)
                    .map_err(|e| format!("Failed to copy {from_name}: {e}"))?;
            }
        }
    }
    mark_done("ccswitch");
    Ok(true)
}

/// Read the Tako CLI apiKey from ~/.tako (after user confirms). Returns the
/// cr_ key so the frontend can establish the Tako login / remote daemon.
#[tauri::command]
pub async fn migration_import_tako_cli() -> Result<String, String> {
    let tako_config = home().join(".tako").join("config.json");
    let text = fs::read_to_string(&tako_config)
        .map_err(|e| format!("Failed to read ~/.tako/config.json: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Invalid Tako config: {e}"))?;
    let api_key = json
        .get("apiKey")
        .and_then(|v| v.as_str())
        .ok_or("No apiKey in Tako config")?
        .to_string();
    mark_done("tako-cli");
    Ok(api_key)
}

#[derive(Serialize, Default)]
pub struct TakoLoginResult {
    pub ok: bool,
    pub name: Option<String>,
    pub plan: Option<String>,
    pub error: Option<String>,
}

/// Validate a Tako `cr_` key against par's identity endpoint. Used by the
/// login screen (user pastes a key). The public gateway is used since the
/// desktop app runs on end-user machines, not inside the mesh.
#[tauri::command]
pub async fn tako_login(apiKey: String) -> Result<TakoLoginResult, String> {
    let url = "https://tako.shiroha.tech/apiStats/api/verify-identity";
    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .json(&serde_json::json!({ "apiKey": apiKey }))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Bad response: {e}"))?;

    if json.get("success").and_then(|v| v.as_bool()) == Some(true) {
        let user = json.get("user");
        Ok(TakoLoginResult {
            ok: true,
            name: user
                .and_then(|u| u.get("name"))
                .and_then(|v| v.as_str())
                .map(String::from),
            plan: user
                .and_then(|u| u.get("plan"))
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
                .map(String::from),
            error: None,
        })
    } else {
        Ok(TakoLoginResult {
            ok: false,
            error: json
                .get("error")
                .and_then(|v| v.as_str())
                .map(String::from)
                .or(Some("Invalid key".into())),
            ..Default::default()
        })
    }
}
