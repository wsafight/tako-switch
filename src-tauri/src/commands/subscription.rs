use std::str::FromStr;
use tauri::{Emitter, State};

use crate::app_config::AppType;
use crate::services::subscription::{CredentialStatus, SubscriptionQuota};
use crate::store::AppState;

/// 查询官方订阅额度
///
/// 读取 CLI 工具已有的 OAuth 凭据并调用官方 API 获取使用额度。
/// 结果（无论业务失败还是 transport 层 Err）都会写入 `UsageCache`、通知托盘
/// 刷新，并 emit `usage-cache-updated`，让前端 React Query 与托盘共享同一份
/// 最新数据。失败快照写入后 `format_subscription_summary` 会通过 `success=false`
/// 守卫返回 `None`，托盘 suffix 自然消失，避免长期滞留旧配额数字。
/// Err 原样向前端返回，React Query 的 onError 不会被吞掉。
#[tauri::command]
pub async fn get_subscription_quota(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    tool: String,
) -> Result<SubscriptionQuota, String> {
    // Tako built-in provider: fetch usage from par (5h / daily / weekly) and
    // reuse the existing SubscriptionQuotaFooter rendering, instead of the
    // file-based CLI credential path.
    if let Some(tako) = tako_quota_if_current(&state, &tool).await {
        let snapshot = tako.clone();
        if let Ok(app_type) = AppType::from_str(&tool) {
            let payload = serde_json::json!({
                "kind": "subscription",
                "appType": app_type.as_str(),
                "data": &snapshot,
            });
            let _ = app.emit("usage-cache-updated", payload);
            state.usage_cache.put_subscription(app_type, snapshot);
            crate::tray::schedule_tray_refresh(&app);
        }
        return Ok(tako);
    }

    let inner = crate::services::subscription::get_subscription_quota(&tool).await;
    let snapshot = match &inner {
        Ok(q) => q.clone(),
        // transport 层 Err —— 凭据状态不明，用 Valid 表达"凭据没问题，是通信/parse 出错"。
        Err(err_msg) => SubscriptionQuota::error(&tool, CredentialStatus::Valid, err_msg.clone()),
    };
    if let Ok(app_type) = AppType::from_str(&tool) {
        let payload = serde_json::json!({
            "kind": "subscription",
            "appType": app_type.as_str(),
            "data": &snapshot,
        });
        if let Err(e) = app.emit("usage-cache-updated", payload) {
            log::error!("emit usage-cache-updated (subscription) 失败: {e}");
        }
        state.usage_cache.put_subscription(app_type, snapshot);
        crate::tray::schedule_tray_refresh(&app);
    }
    inner
}

/// If the current provider for `tool` is the Tako built-in, fetch its usage
/// from par and build a SubscriptionQuota (5h / daily / weekly tiers).
/// Returns None when the active provider is not Tako (fall back to CLI path).
async fn tako_quota_if_current(
    state: &State<'_, AppState>,
    tool: &str,
) -> Option<SubscriptionQuota> {
    use crate::database::TAKO_PROVIDER_ID;
    use crate::services::subscription::{QuotaTier, TIER_FIVE_HOUR, TIER_WEEKLY_LIMIT};

    let current = state.db.get_current_provider(tool).ok().flatten()?;
    if current != TAKO_PROVIDER_ID {
        return None;
    }
    let provider = state
        .db
        .get_provider_by_id(TAKO_PROVIDER_ID, tool)
        .ok()
        .flatten()?;
    let cfg = &provider.settings_config;
    // Pull the cr_ key from whichever auth field this app uses.
    let key = cfg
        .get("env")
        .and_then(|e| e.get("ANTHROPIC_AUTH_TOKEN"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            cfg.get("auth")
                .and_then(|a| a.get("OPENAI_API_KEY"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            cfg.get("env")
                .and_then(|e| e.get("GEMINI_API_KEY"))
                .and_then(|v| v.as_str())
        })
        .filter(|s| !s.is_empty())?;

    let usage = crate::commands::tako_usage(key.to_string()).await.ok()?;
    if !usage.ok {
        return None;
    }

    let pct = |w: &crate::commands::TakoUsageWindow| {
        if w.limit > 0.0 {
            ((w.used / w.limit) * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        }
    };

    let tiers = vec![
        QuotaTier {
            name: TIER_FIVE_HOUR.to_string(),
            utilization: pct(&usage.window),
            resets_at: None,
            used_value_usd: Some(usage.window.used),
            max_value_usd: Some(usage.window.limit),
        },
        QuotaTier {
            name: "daily_limit".to_string(),
            utilization: pct(&usage.daily),
            resets_at: None,
            used_value_usd: Some(usage.daily.used),
            max_value_usd: Some(usage.daily.limit),
        },
        QuotaTier {
            name: TIER_WEEKLY_LIMIT.to_string(),
            utilization: pct(&usage.weekly),
            resets_at: None,
            used_value_usd: Some(usage.weekly.used),
            max_value_usd: Some(usage.weekly.limit),
        },
    ];

    Some(SubscriptionQuota {
        tool: tool.to_string(),
        credential_status: CredentialStatus::Valid,
        credential_message: None,
        success: true,
        tiers,
        extra_usage: None,
        error: None,
        queried_at: Some(chrono::Utc::now().timestamp_millis()),
    })
}
