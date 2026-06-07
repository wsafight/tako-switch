//! Self-contained statusline renderer for Claude Code, styled to match the
//! Tako CLI statusline exactly (same emojis, ANSI colors, ` │ ` separator).
//!
//!   📁 ~/ccgo │ 🌿 main ✓ │ 🤖 Opus 4.5 │ ⚡73% │ 💰 Session:$0.30
//!
//! Invoked as `tako-switch statusline`; Claude Code pipes JSON on stdin. No
//! dependency on the Tako CLI.

use std::io::Read;

// ANSI — mirror packages/cli/src/statusline/colors.ts
const RESET: &str = "\x1b[0m";
const BRIGHT_GREEN: &str = "\x1b[92m";
const BRIGHT_YELLOW: &str = "\x1b[93m";
const BRIGHT_RED: &str = "\x1b[91m";
const BRIGHT_BLUE: &str = "\x1b[94m";
const BRIGHT_MAGENTA: &str = "\x1b[95m";
const BRIGHT_CYAN: &str = "\x1b[96m";
const BRIGHT_BLACK: &str = "\x1b[90m";

pub fn render_statusline() {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let json: serde_json::Value = serde_json::from_str(&input).unwrap_or_default();

    let sep = format!("{BRIGHT_BLACK} │ {RESET}");
    let mut parts: Vec<String> = Vec::new();

    if let Some(s) = seg_directory(&json) {
        parts.push(s);
    }
    if let Some(s) = seg_git(&json) {
        parts.push(s);
    }
    if let Some(s) = seg_model(&json) {
        parts.push(s);
    }
    if let Some(s) = seg_context(&json) {
        parts.push(s);
    }
    if let Some(s) = seg_cost(&json) {
        parts.push(s);
    }

    println!("{}", parts.join(&sep));
}

fn seg_directory(json: &serde_json::Value) -> Option<String> {
    let dir = json.get("workspace")?.get("current_dir")?.as_str()?;
    let display = match std::env::var("HOME") {
        Ok(home) if dir == home => "~".to_string(),
        Ok(home) if dir.starts_with(&home) => format!("~{}", &dir[home.len()..]),
        _ => dir.to_string(),
    };
    Some(format!(
        "{BRIGHT_YELLOW}\u{1F4C1}{RESET} {BRIGHT_GREEN}{display}{RESET}"
    ))
}

fn seg_git(json: &serde_json::Value) -> Option<String> {
    let dir = json.get("workspace")?.get("current_dir")?.as_str()?;
    let head = std::path::Path::new(dir).join(".git").join("HEAD");
    let content = std::fs::read_to_string(head).ok()?;
    let branch = content.trim().strip_prefix("ref: refs/heads/")?;
    Some(format!(
        "{BRIGHT_BLUE}\u{1F33F}{RESET} {BRIGHT_BLUE}{branch}{RESET}"
    ))
}

fn seg_model(json: &serde_json::Value) -> Option<String> {
    let name = json.get("model")?.get("display_name")?.as_str()?;
    Some(format!(
        "{BRIGHT_CYAN}\u{1F916}{RESET} {BRIGHT_CYAN}{name}{RESET}"
    ))
}

/// Remaining context window % — mirrors cli/statusline/segments/context.ts.
/// Reads the transcript JSONL, finds the last assistant usage, sums tokens.
fn seg_context(json: &serde_json::Value) -> Option<String> {
    let path = json.get("transcript_path")?.as_str()?;
    let content = std::fs::read_to_string(path).ok()?;
    let mut tokens: Option<u64> = None;
    for line in content.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if entry.get("type").and_then(|v| v.as_str()) == Some("assistant") {
            if let Some(u) = entry.get("message").and_then(|m| m.get("usage")) {
                let g = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                tokens = Some(
                    g("input_tokens")
                        + g("output_tokens")
                        + g("cache_creation_input_tokens")
                        + g("cache_read_input_tokens"),
                );
                break;
            }
        }
    }
    let tokens = tokens?;
    const LIMIT: u64 = 200_000;
    let used_pct = ((tokens as f64 / LIMIT as f64) * 100.0).round() as i64;
    let remaining = (100 - used_pct).max(0);
    let color = if remaining <= 20 {
        BRIGHT_RED
    } else if remaining <= 50 {
        BRIGHT_YELLOW
    } else {
        BRIGHT_GREEN
    };
    Some(format!(
        "{BRIGHT_MAGENTA}\u{26A1}{RESET} {color}{remaining}%{RESET}"
    ))
}

fn seg_cost(json: &serde_json::Value) -> Option<String> {
    let cost = json.get("cost")?.get("total_cost_usd")?.as_f64()?;
    if cost <= 0.0 {
        return None;
    }
    Some(format!(
        "{BRIGHT_MAGENTA}\u{1F4B0}{RESET} {BRIGHT_GREEN}Session:${cost:.2}{RESET}"
    ))
}
