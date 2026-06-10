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
use std::str::FromStr;
use tauri::State;

use crate::app_config::AppType;
use crate::database::TAKO_PROVIDER_ID;
use crate::store::AppState;

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
    let has_ccswitch_data =
        ccswitch_dir.join("cc-switch.db").exists() || ccswitch_dir.join("config.json").exists();
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
    for (from_name, to_name) in [
        ("config.json", "config.json"),
        ("cc-switch.db", "tako-switch.db"),
    ] {
        let from = src.join(from_name);
        if from.exists() {
            let to = dst.join(to_name);
            // Don't clobber an existing Tako file the user already created.
            if !to.exists() {
                fs::copy(&from, &to).map_err(|e| format!("Failed to copy {from_name}: {e}"))?;
            }
        }
    }
    mark_done("ccswitch");
    Ok(true)
}

/// Read the Tako CLI apiKey from ~/.tako and write it into the built-in Tako
/// provider's auth field for each app, so the user is logged in immediately.
#[tauri::command]
pub async fn migration_import_tako_cli(state: State<'_, AppState>) -> Result<String, String> {
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

    // Write the key into the Tako provider of each app.
    write_key_into_tako_providers(&state, &api_key);

    mark_done("tako-cli");
    Ok(api_key)
}

/// 把 cr_ key 写进 claude/codex/gemini 三个内置 Tako provider 的 auth 字段。
/// 迁移导入与 OAuth 授权回调共用此逻辑。返回成功写入的 app 数。
pub(crate) fn write_key_into_tako_providers(state: &AppState, api_key: &str) -> usize {
    let mut written = 0usize;
    for app in ["claude", "codex", "gemini"] {
        let app_type = match AppType::from_str(app) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if let Ok(Some(mut provider)) = state.db.get_provider_by_id(TAKO_PROVIDER_ID, app) {
            let cfg = &mut provider.settings_config;
            match app {
                "claude" => {
                    cfg["env"]["ANTHROPIC_AUTH_TOKEN"] =
                        serde_json::Value::String(api_key.to_string());
                    cfg["env"]["ANTHROPIC_BASE_URL"] =
                        serde_json::Value::String("https://tako.shiroha.tech".to_string());
                }
                "codex" => {
                    cfg["auth"]["OPENAI_API_KEY"] = serde_json::Value::String(api_key.to_string());
                }
                "gemini" => {
                    cfg["env"]["GEMINI_API_KEY"] = serde_json::Value::String(api_key.to_string());
                }
                _ => {}
            }
            if state.db.save_provider(app_type.as_str(), &provider).is_ok() {
                written += 1;
            }
        }
    }
    written
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

/// OAuth 授权回调落地：验证 cr_ key，通过后写入三个 Tako provider。
/// 由前端在收到 `tako-auth` 深链事件并校验 state 后调用。
#[tauri::command]
pub async fn tako_apply_key(
    state: State<'_, AppState>,
    api_key: String,
) -> Result<TakoLoginResult, String> {
    let result = tako_login(api_key.clone()).await?;
    if result.ok {
        write_key_into_tako_providers(&state, &api_key);
    }
    Ok(result)
}

/// 读取当前已登录的 cr_ key（claude 的 Tako provider auth 字段），非空返回 Some。
pub(crate) fn current_tako_key(state: &AppState) -> Option<String> {
    let provider = state
        .db
        .get_provider_by_id(TAKO_PROVIDER_ID, "claude")
        .ok()
        .flatten()?;
    provider.settings_config["env"]["ANTHROPIC_AUTH_TOKEN"]
        .as_str()
        .map(str::to_string)
        .filter(|k| !k.is_empty())
}

#[derive(Serialize, Default)]
pub struct TakoIdentity {
    pub logged_in: bool,
    pub name: Option<String>,
    pub plan: Option<String>,
    /// 有 key 但验证请求失败（如离线）：仍视为已登录，避免网络抖动登出。
    pub offline: bool,
}

/// 返回当前登录身份：从 Tako provider 读 key → verify-identity。
/// 无 key = 未登录；有 key 但网络失败 = 已登录(offline)。
#[tauri::command]
pub async fn tako_current_identity(state: State<'_, AppState>) -> Result<TakoIdentity, String> {
    let Some(key) = current_tako_key(&state) else {
        return Ok(TakoIdentity::default());
    };
    match tako_login(key).await {
        Ok(r) if r.ok => Ok(TakoIdentity {
            logged_in: true,
            name: r.name,
            plan: r.plan,
            offline: false,
        }),
        // 验证明确失败（key 失效）→ 仍视为已登录但无资料？不：key 失效应判未登录。
        Ok(_) => Ok(TakoIdentity::default()),
        // 网络错误 → 有 key，离线视为已登录。
        Err(_) => Ok(TakoIdentity {
            logged_in: true,
            offline: true,
            ..Default::default()
        }),
    }
}

/// 登出：清空三个 Tako provider 的 auth 字段。
#[tauri::command]
pub async fn tako_logout(state: State<'_, AppState>) -> Result<bool, String> {
    write_key_into_tako_providers(&state, "");
    Ok(true)
}

#[derive(Serialize, Default)]
pub struct TakoUsageWindow {
    pub used: f64,
    pub limit: f64,
}

#[derive(Serialize, Default)]
pub struct TakoUsage {
    pub ok: bool,
    /// 5-hour rolling window.
    pub window: TakoUsageWindow,
    pub daily: TakoUsageWindow,
    pub weekly: TakoUsageWindow,
    pub plan_name: Option<String>,
    pub error: Option<String>,
}

/// Fetch the user's 5h / daily / weekly usage from par for the given cr_ key.
/// Flow: get-key-id (key -> apiId) then user-quota (apiId -> usage + limits).
#[tauri::command]
pub async fn tako_usage(apiKey: String) -> Result<TakoUsage, String> {
    let base = "https://tako.shiroha.tech/apiStats/api";
    let client = reqwest::Client::new();

    // 1. key -> apiId
    let kid: serde_json::Value = client
        .post(format!("{base}/get-key-id"))
        .json(&serde_json::json!({ "apiKey": apiKey }))
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?
        .json()
        .await
        .map_err(|e| format!("bad get-key-id: {e}"))?;
    let api_id = kid
        .get("data")
        .and_then(|d| d.get("id"))
        .and_then(|v| v.as_str());
    let Some(api_id) = api_id else {
        return Ok(TakoUsage {
            ok: false,
            error: Some("Invalid key".into()),
            ..Default::default()
        });
    };

    // 2. apiId -> quota
    let q: serde_json::Value = client
        .get(format!("{base}/user-quota?apiId={api_id}"))
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?
        .json()
        .await
        .map_err(|e| format!("bad user-quota: {e}"))?;

    let usage = q.get("usage");
    let plan = q.get("plan");
    let f = |o: Option<&serde_json::Value>, k: &str| {
        o.and_then(|v| v.get(k))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    };

    Ok(TakoUsage {
        ok: true,
        window: TakoUsageWindow {
            used: f(usage, "windowCost"),
            limit: f(plan, "window_cost_limit"),
        },
        daily: TakoUsageWindow {
            used: f(usage, "dailyCost"),
            limit: f(plan, "daily_cost_limit"),
        },
        weekly: TakoUsageWindow {
            used: f(usage, "weeklyCost"),
            limit: f(plan, "weekly_cost_limit"),
        },
        plan_name: plan
            .and_then(|p| p.get("name"))
            .and_then(|v| v.as_str())
            .map(String::from),
        error: None,
    })
}

#[derive(Serialize, Default)]
pub struct TakoModel {
    pub id: String,
    pub name: String,
    pub provider: String,
    /// 适用客户端（从厂商推断）：claude / codex / gemini。
    pub clients: Vec<String>,
}

/// 从厂商名推断该模型适用的 Tako 客户端。
fn clients_for_provider(provider: &str) -> Vec<String> {
    let p = provider.to_lowercase();
    let mut out = Vec::new();
    if p.contains("anthropic") || p.contains("claude") {
        out.push("claude".to_string());
    }
    if p.contains("openai") || p.contains("gpt") || p.contains("codex") {
        out.push("codex".to_string());
    }
    if p.contains("google") || p.contains("gemini") {
        out.push("gemini".to_string());
    }
    out
}

/// 列出 Tako 支持的模型（用 cr_ key 调网关 /v1/models，OpenAI 格式）。
#[tauri::command]
pub async fn tako_list_models(apiKey: String) -> Result<Vec<TakoModel>, String> {
    let url = "https://tako.shiroha.tech/v1/models";
    let client = reqwest::Client::new();
    let resp: serde_json::Value = client
        .get(url)
        .bearer_auth(&apiKey)
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?
        .json()
        .await
        .map_err(|e| format!("bad models response: {e}"))?;

    let data = resp
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or("no model list in response")?;

    Ok(parse_models(data))
}

/// 把 /v1/models 的 OpenAI 格式 data[] 解析成 TakoModel 列表。
fn parse_models(data: &[serde_json::Value]) -> Vec<TakoModel> {
    data.iter()
        .filter_map(|m| {
            let id = m.get("id").and_then(|v| v.as_str())?.to_string();
            let provider = m
                .get("owned_by")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let clients = clients_for_provider(&provider);
            Some(TakoModel {
                name: id.clone(),
                clients,
                id,
                provider,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use std::sync::Arc;

    /// 授权回调 / 迁移共用的写入逻辑：cr_ key 必须落进三个 Tako provider 的 auth 字段。
    #[test]
    fn write_key_lands_in_all_tako_providers() {
        let db = Arc::new(Database::memory().expect("memory db"));
        db.init_tako_providers().expect("seed tako providers");
        let state = AppState::new(db);

        let written = write_key_into_tako_providers(&state, "cr_secret123");
        assert_eq!(written, 3, "should write claude/codex/gemini");

        let claude = state
            .db
            .get_provider_by_id(TAKO_PROVIDER_ID, "claude")
            .unwrap()
            .unwrap();
        assert_eq!(
            claude.settings_config["env"]["ANTHROPIC_AUTH_TOKEN"],
            "cr_secret123"
        );

        let codex = state
            .db
            .get_provider_by_id(TAKO_PROVIDER_ID, "codex")
            .unwrap()
            .unwrap();
        assert_eq!(
            codex.settings_config["auth"]["OPENAI_API_KEY"],
            "cr_secret123"
        );

        let gemini = state
            .db
            .get_provider_by_id(TAKO_PROVIDER_ID, "gemini")
            .unwrap()
            .unwrap();
        assert_eq!(
            gemini.settings_config["env"]["GEMINI_API_KEY"],
            "cr_secret123"
        );
    }

    #[test]
    fn infers_clients_from_provider() {
        assert_eq!(clients_for_provider("Anthropic"), vec!["claude"]);
        assert_eq!(clients_for_provider("OpenAI"), vec!["codex"]);
        assert_eq!(clients_for_provider("Google"), vec!["gemini"]);
        assert!(clients_for_provider("Unknown").is_empty());
    }

    #[test]
    fn parses_openai_models_payload() {
        let data = serde_json::json!([
            { "id": "claude-opus-4", "owned_by": "Anthropic", "object": "model" },
            { "id": "gpt-5", "owned_by": "OpenAI" },
            { "object": "model" }
        ]);
        let models = parse_models(data.as_array().unwrap());
        assert_eq!(models.len(), 2, "skips entries without id");
        assert_eq!(models[0].id, "claude-opus-4");
        assert_eq!(models[0].name, "claude-opus-4");
        assert_eq!(models[0].clients, vec!["claude"]);
        assert_eq!(models[1].clients, vec!["codex"]);
    }

    #[test]
    fn current_key_reflects_login_and_logout() {
        let db = Arc::new(Database::memory().expect("memory db"));
        db.init_tako_providers().expect("seed tako providers");
        let state = AppState::new(db);

        // 未写 key → 未登录。
        assert!(current_tako_key(&state).is_none());

        // 写 key → 已登录。
        write_key_into_tako_providers(&state, "cr_abc");
        assert_eq!(current_tako_key(&state).as_deref(), Some("cr_abc"));

        // 清空（登出）→ 未登录。
        write_key_into_tako_providers(&state, "");
        assert!(current_tako_key(&state).is_none());
    }
}
