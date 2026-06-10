use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;
use sha2::{Digest, Sha256};

const VALID_TOOLS: [&str; 6] = [
    "claude", "codex", "gemini", "opencode", "openclaw", "hermes",
];

const OFFICIAL_INSTALL_DOMAINS: [&str; 6] = [
    "claude.ai",
    "chatgpt.com",
    "persistent.oaistatic.com",
    "get.microsoft.com",
    "opencode.ai",
    "github.com",
];

const NETWORK_CACHE_TTL_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallWslShellPreferenceInput {
    #[serde(default)]
    pub wsl_shell: Option<String>,
    #[serde(default)]
    pub wsl_shell_flag: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OfficialDomainProbe {
    domain: String,
    ok: bool,
    latency_ms: Option<u128>,
    error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PackageManagerDetection {
    node: bool,
    npm: bool,
    npx: bool,
    bun: bool,
    brew: bool,
    winget: bool,
    paru: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EnvironmentToolStatus {
    tool: String,
    installed: bool,
    version: Option<String>,
    installed_but_broken: bool,
    env_type: String,
    wsl_distro: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RecommendedInstallSource {
    tool: String,
    platform: String,
    arch: String,
    source: String,
    command: Option<String>,
    url: Option<String>,
    version: Option<String>,
    checksum: Option<String>,
    official_url: Option<String>,
    manual_url: Option<String>,
    warning: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct InstallCliRequest {
    tool: String,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ConfigureToolRequest {
    tool: String,
    gateway_url: String,
    api_key: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InstallSourceTestResult {
    tool: String,
    source: String,
    ok: bool,
    command: Option<String>,
    url: Option<String>,
    downloaded_bytes: Option<u64>,
    status: Option<u16>,
    error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InstallCliResult {
    tool: String,
    dry_run: bool,
    command: Option<String>,
    skipped: bool,
    before: EnvironmentToolStatus,
    after: EnvironmentToolStatus,
    stdout_tail: Option<String>,
    stderr_tail: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConfigureToolResult {
    tool: String,
    configured: bool,
    path: Option<String>,
    restart_required: bool,
    error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EnvironmentDetection {
    os: String,
    arch: String,
    region_hint: String,
    network_region: String,
    source: String,
    official_domains_reachable: Vec<OfficialDomainProbe>,
    blocked_domains: Vec<String>,
    package_managers: PackageManagerDetection,
    tools: Vec<EnvironmentToolStatus>,
    recommended_sources: Vec<RecommendedInstallSource>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct InstallNetworkCache {
    region_hint: String,
    network_region: String,
    official_domains_reachable: Vec<OfficialDomainProbe>,
    blocked_domains: Vec<String>,
    detected_at: u64,
}

#[derive(Debug, Clone)]
struct InstallNetworkDetection {
    region_hint: String,
    network_region: String,
    source: String,
    official_domains_reachable: Vec<OfficialDomainProbe>,
    blocked_domains: Vec<String>,
}

#[tauri::command]
pub async fn detect_environment(
    tools: Option<Vec<String>>,
    force_refresh: Option<bool>,
    wsl_shell_by_tool: Option<HashMap<String, InstallWslShellPreferenceInput>>,
) -> Result<EnvironmentDetection, String> {
    let requested = normalize_requested_tools(tools.as_deref());
    let os = current_install_platform();
    let arch = current_install_arch();
    let package_managers = detect_package_managers();
    let local_region = local_region_hint();

    let tool_statuses = tokio::task::spawn_blocking(move || {
        requested
            .into_iter()
            .map(|tool| detect_tool_status(tool, wsl_shell_by_tool.as_ref()))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| format!("detect environment task join error: {e}"))?;

    let network = detect_install_network(force_refresh.unwrap_or(false), &local_region).await;
    let recommended_sources =
        recommended_install_sources(&os, &arch, &network, &package_managers, &tool_statuses);

    Ok(EnvironmentDetection {
        os,
        arch,
        region_hint: network.region_hint,
        network_region: network.network_region,
        source: network.source,
        official_domains_reachable: network.official_domains_reachable,
        blocked_domains: network.blocked_domains,
        package_managers,
        tools: tool_statuses,
        recommended_sources,
    })
}

#[tauri::command]
pub async fn test_install_source(tool: String) -> Result<InstallSourceTestResult, String> {
    let tool = normalize_single_tool(&tool)?;
    let platform = current_install_platform();
    let arch = current_install_arch();
    let package_managers = detect_package_managers();
    let local_region = local_region_hint();
    let network = detect_install_network(false, &local_region).await;
    let restricted = network.network_region == "restricted" || network.region_hint == "cn";
    let source = recommend_for_tool(tool, &platform, &arch, restricted, &package_managers);
    test_recommended_source(source).await
}

#[tauri::command]
pub async fn install_cli_tool(request: InstallCliRequest) -> Result<InstallCliResult, String> {
    let tool = normalize_single_tool(&request.tool)?;
    install_single_cli_tool(tool, request.dry_run, request.force).await
}

#[tauri::command]
pub async fn install_cli_tools(
    requests: Vec<InstallCliRequest>,
) -> Result<Vec<InstallCliResult>, String> {
    let mut results = Vec::new();
    for request in requests {
        let tool = normalize_single_tool(&request.tool)?;
        results.push(install_single_cli_tool(tool, request.dry_run, request.force).await?);
    }
    Ok(results)
}

#[tauri::command]
pub async fn configure_installed_tool(
    request: ConfigureToolRequest,
) -> Result<ConfigureToolResult, String> {
    let tool = normalize_single_tool(&request.tool)?;
    configure_tool(tool, &request.gateway_url, &request.api_key)
}

#[tauri::command]
pub async fn configure_installed_tools(
    requests: Vec<ConfigureToolRequest>,
) -> Result<Vec<ConfigureToolResult>, String> {
    let mut results = Vec::new();
    for request in requests {
        let tool = normalize_single_tool(&request.tool)?;
        results.push(configure_tool(
            tool,
            &request.gateway_url,
            &request.api_key,
        )?);
    }
    Ok(results)
}

fn normalize_requested_tools(tools: Option<&[String]>) -> Vec<&'static str> {
    if let Some(tools) = tools {
        let set: std::collections::HashSet<&str> = tools.iter().map(|s| s.as_str()).collect();
        VALID_TOOLS
            .iter()
            .copied()
            .filter(|tool| set.contains(tool))
            .collect()
    } else {
        VALID_TOOLS.to_vec()
    }
}

fn normalize_single_tool(tool: &str) -> Result<&'static str, String> {
    VALID_TOOLS
        .iter()
        .copied()
        .find(|candidate| *candidate == tool)
        .ok_or_else(|| format!("Unsupported tool: {tool}"))
}

fn current_install_platform() -> String {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
    .to_string()
}

fn current_install_arch() -> String {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86_64") {
        "x64"
    } else {
        "unknown"
    }
    .to_string()
}

fn detect_package_managers() -> PackageManagerDetection {
    PackageManagerDetection {
        node: command_exists("node"),
        npm: command_exists("npm"),
        npx: command_exists("npx"),
        bun: command_exists("bun"),
        brew: command_exists("brew"),
        winget: command_exists("winget"),
        paru: command_exists("paru"),
    }
}

fn command_exists(command: &str) -> bool {
    #[cfg(target_os = "windows")]
    let output = Command::new("where").arg(command).output();

    #[cfg(not(target_os = "windows"))]
    let output = Command::new("which").arg(command).output();

    output.map(|out| out.status.success()).unwrap_or(false)
}

fn detect_tool_status(
    tool: &str,
    _wsl_shell_by_tool: Option<&HashMap<String, InstallWslShellPreferenceInput>>,
) -> EnvironmentToolStatus {
    let env_type = current_install_platform();
    match run_version_command(tool) {
        Ok(version) => EnvironmentToolStatus {
            tool: tool.to_string(),
            installed: true,
            version: Some(extract_version(&version)),
            installed_but_broken: false,
            env_type,
            wsl_distro: None,
            error: None,
        },
        Err(error) if command_exists(tool) => EnvironmentToolStatus {
            tool: tool.to_string(),
            installed: true,
            version: None,
            installed_but_broken: true,
            env_type,
            wsl_distro: None,
            error: Some(error),
        },
        Err(error) => EnvironmentToolStatus {
            tool: tool.to_string(),
            installed: false,
            version: None,
            installed_but_broken: false,
            env_type,
            wsl_distro: None,
            error: Some(error),
        },
    }
}

fn run_version_command(tool: &str) -> Result<String, String> {
    let output = Command::new(tool)
        .arg("--version")
        .output()
        .map_err(|e| format!("not installed or not executable: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        let raw = if stdout.is_empty() { stderr } else { stdout };
        if raw.is_empty() {
            Err("version command returned empty output".to_string())
        } else {
            Ok(raw)
        }
    } else {
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

fn extract_version(raw: &str) -> String {
    raw.split_whitespace()
        .find(|part| part.chars().any(|ch| ch.is_ascii_digit()))
        .unwrap_or(raw)
        .trim()
        .to_string()
}

fn local_region_hint() -> String {
    let tz = std::env::var("TZ").unwrap_or_default().to_ascii_lowercase();
    let lang = std::env::var("LANG")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if tz.contains("shanghai")
        || tz.contains("chongqing")
        || tz.contains("urumqi")
        || lang.contains("zh_cn")
    {
        "cn".to_string()
    } else if !tz.is_empty() || !lang.is_empty() {
        "global".to_string()
    } else {
        "unknown".to_string()
    }
}

async fn detect_install_network(
    force_refresh: bool,
    local_region: &str,
) -> InstallNetworkDetection {
    if let Some(region) = env_region_override() {
        let network_region = if region == "cn" {
            "restricted"
        } else {
            "unknown"
        };
        return InstallNetworkDetection {
            region_hint: region,
            network_region: network_region.to_string(),
            source: "env".to_string(),
            official_domains_reachable: Vec::new(),
            blocked_domains: Vec::new(),
        };
    }

    if !force_refresh {
        if let Some(cache) = read_network_cache() {
            return InstallNetworkDetection {
                region_hint: cache.region_hint,
                network_region: cache.network_region,
                source: "cache".to_string(),
                official_domains_reachable: cache.official_domains_reachable,
                blocked_domains: cache.blocked_domains,
            };
        }
    }

    let mut probes = Vec::new();
    for domain in OFFICIAL_INSTALL_DOMAINS {
        probes.push(probe_domain(domain).await);
    }

    let blocked_domains = probes
        .iter()
        .filter(|probe| !probe.ok)
        .map(|probe| probe.domain.clone())
        .collect::<Vec<_>>();
    let ok_count = probes.iter().filter(|probe| probe.ok).count();
    let network_region = if ok_count >= 4 {
        "normal"
    } else if blocked_domains.len() >= 3 {
        "restricted"
    } else {
        "unknown"
    };
    let region_hint = if local_region == "unknown" && network_region == "restricted" {
        "cn".to_string()
    } else {
        local_region.to_string()
    };

    let detection = InstallNetworkDetection {
        region_hint,
        network_region: network_region.to_string(),
        source: "official_probe".to_string(),
        official_domains_reachable: probes,
        blocked_domains,
    };
    let _ = write_network_cache(&detection);
    detection
}

fn env_region_override() -> Option<String> {
    for key in ["TAKO_SWITCH_REGION", "TAKO_REGION"] {
        if let Ok(value) = std::env::var(key) {
            match value.trim().to_ascii_lowercase().as_str() {
                "cn" => return Some("cn".to_string()),
                "global" => return Some("global".to_string()),
                "auto" => return None,
                _ => {}
            }
        }
    }
    None
}

async fn probe_domain(domain: &str) -> OfficialDomainProbe {
    let url = format!("https://{domain}/");
    let client = crate::proxy::http_client::get();
    let started = Instant::now();
    let result = tokio::time::timeout(Duration::from_millis(1800), client.head(&url).send()).await;
    match result {
        Ok(Ok(response)) => OfficialDomainProbe {
            domain: domain.to_string(),
            ok: response.status().is_success() || response.status().is_redirection(),
            latency_ms: Some(started.elapsed().as_millis()),
            error: None,
        },
        Ok(Err(err)) => OfficialDomainProbe {
            domain: domain.to_string(),
            ok: false,
            latency_ms: Some(started.elapsed().as_millis()),
            error: Some(err.to_string()),
        },
        Err(_) => OfficialDomainProbe {
            domain: domain.to_string(),
            ok: false,
            latency_ms: Some(started.elapsed().as_millis()),
            error: Some("timeout".to_string()),
        },
    }
}

async fn test_recommended_source(
    source: RecommendedInstallSource,
) -> Result<InstallSourceTestResult, String> {
    if source.source == "manual" {
        return Ok(InstallSourceTestResult {
            tool: source.tool,
            source: source.source,
            ok: false,
            command: source.command,
            url: source.url.or(source.manual_url),
            downloaded_bytes: None,
            status: None,
            error: source
                .warning
                .or_else(|| Some("manual source only".to_string())),
        });
    }

    let probe_url = source
        .url
        .clone()
        .or_else(|| install_script_url_from_command(source.command.as_deref()));
    let Some(url) = probe_url else {
        return Ok(InstallSourceTestResult {
            tool: source.tool,
            source: source.source,
            ok: false,
            command: source.command,
            url: source.url,
            downloaded_bytes: None,
            status: None,
            error: Some("no downloadable URL found for source".to_string()),
        });
    };

    let client = crate::proxy::http_client::get();
    let response = tokio::time::timeout(Duration::from_secs(20), client.get(&url).send()).await;
    match response {
        Ok(Ok(resp)) => {
            let status = resp.status();
            if !status.is_success() && !status.is_redirection() {
                return Ok(InstallSourceTestResult {
                    tool: source.tool,
                    source: source.source,
                    ok: false,
                    command: source.command,
                    url: Some(url),
                    downloaded_bytes: None,
                    status: Some(status.as_u16()),
                    error: Some(format!("HTTP status {status}")),
                });
            }
            let bytes = tokio::time::timeout(Duration::from_secs(20), resp.bytes()).await;
            match bytes {
                Ok(Ok(bytes)) => {
                    let checksum_error = source.checksum.as_deref().and_then(|expected| {
                        let actual = sha256_hex(&bytes);
                        (!actual.eq_ignore_ascii_case(expected.trim())).then(|| {
                            format!(
                                "checksum mismatch: expected {}, got {}",
                                expected.trim(),
                                actual
                            )
                        })
                    });
                    let empty_error = bytes
                        .is_empty()
                        .then(|| "download returned empty body".to_string());
                    let error = checksum_error.or(empty_error);
                    Ok(InstallSourceTestResult {
                        tool: source.tool,
                        source: source.source,
                        ok: error.is_none(),
                        command: source.command,
                        url: Some(url),
                        downloaded_bytes: Some(bytes.len() as u64),
                        status: Some(status.as_u16()),
                        error,
                    })
                }
                Ok(Err(err)) => Ok(InstallSourceTestResult {
                    tool: source.tool,
                    source: source.source,
                    ok: false,
                    command: source.command,
                    url: Some(url),
                    downloaded_bytes: None,
                    status: Some(status.as_u16()),
                    error: Some(err.to_string()),
                }),
                Err(_) => Ok(InstallSourceTestResult {
                    tool: source.tool,
                    source: source.source,
                    ok: false,
                    command: source.command,
                    url: Some(url),
                    downloaded_bytes: None,
                    status: Some(status.as_u16()),
                    error: Some("download timeout".to_string()),
                }),
            }
        }
        Ok(Err(err)) => Ok(InstallSourceTestResult {
            tool: source.tool,
            source: source.source,
            ok: false,
            command: source.command,
            url: Some(url),
            downloaded_bytes: None,
            status: None,
            error: Some(err.to_string()),
        }),
        Err(_) => Ok(InstallSourceTestResult {
            tool: source.tool,
            source: source.source,
            ok: false,
            command: source.command,
            url: Some(url),
            downloaded_bytes: None,
            status: None,
            error: Some("request timeout".to_string()),
        }),
    }
}

async fn install_single_cli_tool(
    tool: &'static str,
    dry_run: bool,
    force: bool,
) -> Result<InstallCliResult, String> {
    let before = detect_tool_status(tool, None);
    let platform = current_install_platform();
    let arch = current_install_arch();
    let package_managers = detect_package_managers();
    let local_region = local_region_hint();
    let network = detect_install_network(false, &local_region).await;
    let restricted = network.network_region == "restricted" || network.region_hint == "cn";
    let source = recommend_for_tool(tool, &platform, &arch, restricted, &package_managers);
    let command = source.command.clone();
    let source_for_install = source.clone();

    if before.installed && !force {
        return Ok(InstallCliResult {
            tool: tool.to_string(),
            dry_run,
            command,
            skipped: true,
            after: before.clone(),
            before,
            stdout_tail: None,
            stderr_tail: None,
            error: None,
        });
    }

    if command.is_none() && source_for_install.url.is_none() {
        return Ok(InstallCliResult {
            tool: tool.to_string(),
            dry_run,
            command,
            skipped: true,
            after: before.clone(),
            before,
            stdout_tail: None,
            stderr_tail: None,
            error: source
                .warning
                .or_else(|| Some("no install command available".to_string())),
        });
    }

    if dry_run {
        return Ok(InstallCliResult {
            tool: tool.to_string(),
            dry_run,
            command,
            skipped: true,
            after: before.clone(),
            before,
            stdout_tail: None,
            stderr_tail: None,
            error: None,
        });
    }

    let output = run_source_install(source_for_install).await;
    let after = detect_tool_status(tool, None);
    Ok(build_install_result(
        tool, false, command, before, after, output,
    ))
}

async fn run_source_install(source: RecommendedInstallSource) -> Result<Output, String> {
    if let Some(command_line) = source.command {
        return tokio::task::spawn_blocking(move || run_install_command(&command_line))
            .await
            .map_err(|e| format!("install task join error: {e}"))?;
    }

    let Some(url) = source.url else {
        return Err("no install command or URL available".to_string());
    };
    let checksum = source
        .checksum
        .ok_or_else(|| "refusing to install URL source without sha256 checksum".to_string())?;
    let script_path = download_verified_installer(&url, &checksum).await?;
    let command_line = shell_command_for_downloaded_installer(&script_path)?;
    let output = tokio::task::spawn_blocking(move || run_install_command(&command_line))
        .await
        .map_err(|e| format!("install task join error: {e}"))?;
    let _ = std::fs::remove_file(script_path);
    output
}

async fn download_verified_installer(url: &str, checksum: &str) -> Result<PathBuf, String> {
    let client = crate::proxy::http_client::get();
    let response = tokio::time::timeout(Duration::from_secs(60), client.get(url).send())
        .await
        .map_err(|_| "download timeout".to_string())?
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("download failed with HTTP {}", response.status()));
    }
    let bytes = tokio::time::timeout(Duration::from_secs(60), response.bytes())
        .await
        .map_err(|_| "download body timeout".to_string())?
        .map_err(|e| e.to_string())?;
    let actual = sha256_hex(&bytes);
    if !actual.eq_ignore_ascii_case(checksum.trim()) {
        return Err(format!(
            "checksum mismatch: expected {}, got {}",
            checksum.trim(),
            actual
        ));
    }
    let path = std::env::temp_dir().join(format!(
        "tako-install-{}-{}",
        std::process::id(),
        now_unix_secs()
    ));
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn shell_command_for_downloaded_installer(path: &PathBuf) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        Ok(format!(
            "powershell -NoProfile -ExecutionPolicy Bypass -File {}",
            windows_quote_arg(&path.to_string_lossy())
        ))
    }

    #[cfg(not(target_os = "windows"))]
    {
        Ok(format!(
            "bash {}",
            shell_single_quote(&path.to_string_lossy())
        ))
    }
}

#[cfg(target_os = "windows")]
fn windows_quote_arg(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

#[cfg(not(target_os = "windows"))]
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(not(target_os = "windows"))]
fn run_install_command(command_line: &str) -> Result<Output, String> {
    Command::new("bash")
        .arg("-c")
        .arg(command_line)
        .output()
        .map_err(|e| format!("启动安装进程失败: {e}"))
}

#[cfg(target_os = "windows")]
fn run_install_command(command_line: &str) -> Result<Output, String> {
    use std::os::windows::process::CommandExt;

    Command::new("cmd")
        .arg("/C")
        .arg(command_line)
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| format!("启动安装进程失败: {e}"))
}

fn build_install_result(
    tool: &str,
    dry_run: bool,
    command: Option<String>,
    before: EnvironmentToolStatus,
    after: EnvironmentToolStatus,
    output: Result<Output, String>,
) -> InstallCliResult {
    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            InstallCliResult {
                tool: tool.to_string(),
                dry_run,
                command,
                skipped: false,
                before,
                after,
                stdout_tail: non_empty_tail(&stdout, 8),
                stderr_tail: non_empty_tail(&stderr, 8),
                error: if output.status.success() {
                    None
                } else {
                    Some(format!(
                        "install command failed: {:?}",
                        output.status.code()
                    ))
                },
            }
        }
        Err(error) => InstallCliResult {
            tool: tool.to_string(),
            dry_run,
            command,
            skipped: false,
            before,
            after,
            stdout_tail: None,
            stderr_tail: None,
            error: Some(error),
        },
    }
}

fn non_empty_tail(text: &str, lines: usize) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parts = trimmed.lines().collect::<Vec<_>>();
    let start = parts.len().saturating_sub(lines);
    Some(parts[start..].join("\n"))
}

fn configure_tool(
    tool: &str,
    gateway_url: &str,
    api_key: &str,
) -> Result<ConfigureToolResult, String> {
    let gateway_url = gateway_url.trim().trim_end_matches('/');
    let api_key = api_key.trim();
    if gateway_url.is_empty() {
        return Err("gateway_url is required".to_string());
    }
    if api_key.is_empty() {
        return Err("api_key is required".to_string());
    }

    match tool {
        "claude" => configure_claude_cli(gateway_url, api_key),
        "codex" => configure_codex_cli(gateway_url, api_key),
        "gemini" => configure_gemini_cli(gateway_url, api_key),
        _ => Ok(ConfigureToolResult {
            tool: tool.to_string(),
            configured: false,
            path: None,
            restart_required: false,
            error: Some(format!("{tool} does not require Tako config in this flow")),
        }),
    }
}

fn configure_claude_cli(gateway_url: &str, api_key: &str) -> Result<ConfigureToolResult, String> {
    #[cfg(target_os = "windows")]
    {
        set_windows_user_env("ANTHROPIC_BASE_URL", &format!("{gateway_url}/api"))?;
        set_windows_user_env("ANTHROPIC_AUTH_TOKEN", api_key)?;
        return Ok(ConfigureToolResult {
            tool: "claude".to_string(),
            configured: true,
            path: Some("User environment variables".to_string()),
            restart_required: true,
            error: None,
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let path = shell_rc_path();
        let block = format!(
            "export ANTHROPIC_BASE_URL=\"{}/api\"\nexport ANTHROPIC_AUTH_TOKEN=\"{}\"",
            shell_escape_double_quoted(gateway_url),
            shell_escape_double_quoted(api_key)
        );
        write_marked_block(&path, "TAKO_SWITCH_CLAUDE", &block)?;
        Ok(ConfigureToolResult {
            tool: "claude".to_string(),
            configured: true,
            path: Some(path.to_string_lossy().to_string()),
            restart_required: true,
            error: None,
        })
    }
}

fn configure_codex_cli(gateway_url: &str, api_key: &str) -> Result<ConfigureToolResult, String> {
    let auth = json!({ "OPENAI_API_KEY": api_key });
    let config = format!(
        "model_provider = \"tako\"\nmodel = \"gpt-5.4\"\n\n[model_providers.tako]\nname = \"tako\"\nbase_url = \"{gateway_url}/v1\"\n"
    );
    crate::codex_config::write_codex_live_atomic(&auth, Some(&config))
        .map_err(|e| e.to_string())?;
    Ok(ConfigureToolResult {
        tool: "codex".to_string(),
        configured: true,
        path: Some(
            crate::codex_config::get_codex_config_dir()
                .to_string_lossy()
                .to_string(),
        ),
        restart_required: true,
        error: None,
    })
}

fn configure_gemini_cli(gateway_url: &str, api_key: &str) -> Result<ConfigureToolResult, String> {
    let mut env = crate::gemini_config::read_gemini_env().map_err(|e| e.to_string())?;
    env.insert("GEMINI_API_KEY".to_string(), api_key.to_string());
    env.insert(
        "GOOGLE_GEMINI_BASE_URL".to_string(),
        gateway_url.to_string(),
    );
    crate::gemini_config::write_gemini_env_atomic(&env).map_err(|e| e.to_string())?;
    Ok(ConfigureToolResult {
        tool: "gemini".to_string(),
        configured: true,
        path: Some(
            crate::gemini_config::get_gemini_env_path()
                .to_string_lossy()
                .to_string(),
        ),
        restart_required: true,
        error: None,
    })
}

#[cfg(target_os = "windows")]
fn set_windows_user_env(key: &str, value: &str) -> Result<(), String> {
    let script = format!(
        "[Environment]::SetEnvironmentVariable('{}', '{}', 'User')",
        key.replace('\'', "''"),
        value.replace('\'', "''")
    );
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()
        .map_err(|e| format!("failed to start PowerShell: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(not(target_os = "windows"))]
fn shell_rc_path() -> PathBuf {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let file = if shell.ends_with("zsh") {
        ".zshrc"
    } else if shell.ends_with("bash") {
        ".bashrc"
    } else {
        ".profile"
    };
    crate::config::get_home_dir().join(file)
}

#[cfg(not(target_os = "windows"))]
fn write_marked_block(path: &PathBuf, marker: &str, block: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let start = format!("# >>> {marker}");
    let end = format!("# <<< {marker}");
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let replacement = format!("{start}\n{block}\n{end}");
    let next = replace_marked_block(&existing, &start, &end, &replacement);
    std::fs::write(path, next).map_err(|e| e.to_string())
}

#[cfg(not(target_os = "windows"))]
fn replace_marked_block(existing: &str, start: &str, end: &str, replacement: &str) -> String {
    if let Some(start_idx) = existing.find(start) {
        if let Some(end_rel) = existing[start_idx..].find(end) {
            let end_idx = start_idx + end_rel + end.len();
            let mut next = String::new();
            next.push_str(existing[..start_idx].trim_end());
            if !next.is_empty() {
                next.push_str("\n\n");
            }
            next.push_str(replacement);
            let tail = existing[end_idx..].trim_start();
            if !tail.is_empty() {
                next.push_str("\n\n");
                next.push_str(tail);
            }
            next.push('\n');
            return next;
        }
    }
    let mut next = existing.trim_end().to_string();
    if !next.is_empty() {
        next.push_str("\n\n");
    }
    next.push_str(replacement);
    next.push('\n');
    next
}

#[cfg(not(target_os = "windows"))]
fn shell_escape_double_quoted(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
}

fn network_cache_path() -> PathBuf {
    crate::config::get_home_dir()
        .join(".tako-switch")
        .join("install-network.json")
}

fn read_network_cache() -> Option<InstallNetworkCache> {
    let path = network_cache_path();
    let raw = std::fs::read_to_string(path).ok()?;
    let cache: InstallNetworkCache = serde_json::from_str(&raw).ok()?;
    let now = now_unix_secs();
    if now.saturating_sub(cache.detected_at) <= NETWORK_CACHE_TTL_SECS {
        Some(cache)
    } else {
        None
    }
}

fn write_network_cache(detection: &InstallNetworkDetection) -> Result<(), String> {
    let path = network_cache_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let cache = InstallNetworkCache {
        region_hint: detection.region_hint.clone(),
        network_region: detection.network_region.clone(),
        official_domains_reachable: detection.official_domains_reachable.clone(),
        blocked_domains: detection.blocked_domains.clone(),
        detected_at: now_unix_secs(),
    };
    let raw = serde_json::to_string_pretty(&cache).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| e.to_string())
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn install_script_url_from_command(command: Option<&str>) -> Option<String> {
    let command = command?;
    for token in
        command.split(|ch: char| ch.is_whitespace() || ch == '\'' || ch == '"' || ch == ';')
    {
        if token.starts_with("https://") || token.starts_with("http://") {
            return Some(token.to_string());
        }
    }
    None
}

fn recommended_install_sources(
    platform: &str,
    arch: &str,
    network: &InstallNetworkDetection,
    package_managers: &PackageManagerDetection,
    tools: &[EnvironmentToolStatus],
) -> Vec<RecommendedInstallSource> {
    let restricted = network.network_region == "restricted" || network.region_hint == "cn";
    tools
        .iter()
        .filter(|tool| !tool.installed)
        .map(|tool| recommend_for_tool(&tool.tool, platform, arch, restricted, package_managers))
        .collect()
}

fn recommend_for_tool(
    tool: &str,
    platform: &str,
    arch: &str,
    restricted: bool,
    package_managers: &PackageManagerDetection,
) -> RecommendedInstallSource {
    recommend_for_tool_with_oss_sources(
        tool,
        platform,
        arch,
        restricted,
        package_managers,
        crate::commands::install_sources::BUILTIN_OSS_SOURCES,
    )
}

fn recommend_for_tool_with_oss_sources(
    tool: &str,
    platform: &str,
    arch: &str,
    restricted: bool,
    package_managers: &PackageManagerDetection,
    oss_sources: &[crate::commands::install_sources::BuiltinOssSource],
) -> RecommendedInstallSource {
    let tool_id = format!("{tool}_cli");
    if restricted {
        if let Some(oss) = crate::commands::install_sources::find_builtin_oss_source_in(
            oss_sources,
            &tool_id,
            platform,
            arch,
        ) {
            return RecommendedInstallSource {
                tool: tool_id,
                platform: platform.to_string(),
                arch: arch.to_string(),
                source: "oss_configured".to_string(),
                command: None,
                url: Some(oss.url.to_string()),
                version: Some(oss.version.to_string()),
                checksum: Some(oss.sha256.to_string()),
                official_url: Some(oss.official_url.to_string()),
                manual_url: Some(oss.official_url.to_string()),
                warning: Some("Using Tako-maintained OSS acceleration source.".to_string()),
            };
        }
    }

    let mut source = RecommendedInstallSource {
        tool: tool_id,
        platform: platform.to_string(),
        arch: arch.to_string(),
        source: "official".to_string(),
        command: None,
        url: None,
        version: Some("latest".to_string()),
        checksum: None,
        official_url: None,
        manual_url: None,
        warning: None,
    };

    match tool {
        "claude" => {
            source.command = match platform {
                "windows" if package_managers.winget => {
                    Some("winget install Anthropic.ClaudeCode".to_string())
                }
                "windows" => None,
                _ => Some("bash -c 'tmp=$(mktemp) && curl -fsSL https://claude.ai/install.sh -o $tmp && bash $tmp; status=$?; rm -f $tmp; exit $status'".to_string()),
            };
            source.manual_url = Some("https://code.claude.com/docs/en/setup".to_string());
        }
        "codex" => {
            source.command = match platform {
                "windows" => Some(
                    "powershell -ExecutionPolicy ByPass -c \"irm https://chatgpt.com/codex/install.ps1 | iex\""
                        .to_string(),
                ),
                _ => Some(
                    "CODEX_NON_INTERACTIVE=1 bash -c 'tmp=$(mktemp) && curl -fsSL https://chatgpt.com/codex/install.sh -o $tmp && bash $tmp; status=$?; rm -f $tmp; exit $status'"
                        .to_string(),
                ),
            };
            source.manual_url = Some("https://developers.openai.com/codex/cli".to_string());
        }
        "gemini" => {
            if package_managers.npm {
                source.command = Some("npm install -g @google/gemini-cli".to_string());
            } else if package_managers.brew {
                source.command = Some("brew install gemini-cli".to_string());
            } else {
                source.source = "manual".to_string();
                source.warning = Some(
                    "Gemini CLI official install requires npm/npx or Homebrew; none detected."
                        .to_string(),
                );
            }
            source.manual_url = Some("https://github.com/google-gemini/gemini-cli".to_string());
        }
        "opencode" => {
            source.command = match platform {
                "windows" if package_managers.npm => Some("npm i -g opencode-ai@latest".to_string()),
                "windows" => None,
                _ => Some("bash -c 'tmp=$(mktemp) && curl -fsSL https://opencode.ai/install -o $tmp && bash $tmp; status=$?; rm -f $tmp; exit $status'".to_string()),
            };
            if source.command.is_none() {
                source.source = "manual".to_string();
            }
            source.manual_url = Some("https://opencode.ai/download".to_string());
        }
        "openclaw" => {
            if package_managers.npm {
                source.command = Some("npm i -g openclaw@latest".to_string());
            } else {
                source.source = "manual".to_string();
                source.warning = Some(
                    "OpenClaw install currently requires npm; npm was not detected.".to_string(),
                );
            }
        }
        "hermes" => {
            source.command = match platform {
                "windows" => Some(
                    "powershell -NoProfile -ExecutionPolicy Bypass -Command \"irm https://raw.githubusercontent.com/NousResearch/hermes-agent/main/scripts/install.ps1 | iex\""
                        .to_string(),
                ),
                _ => Some("bash -c 'tmp=$(mktemp) && curl -fsSL https://raw.githubusercontent.com/NousResearch/hermes-agent/main/scripts/install.sh -o $tmp && bash $tmp; status=$?; rm -f $tmp; exit $status'".to_string()),
            };
            source.manual_url = Some("https://github.com/NousResearch/hermes-agent".to_string());
        }
        _ => {
            source.source = "manual".to_string();
        }
    }

    if restricted && source.source == "official" {
        source.warning = Some(
            "Official install domains may be slow or unreachable on the current network."
                .to_string(),
        );
    }

    source
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn unique_temp_home(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!(
            "tako-switch-install-test-{name}-{}-{suffix}",
            std::process::id()
        ))
    }

    fn no_package_managers() -> PackageManagerDetection {
        PackageManagerDetection {
            node: false,
            npm: false,
            npx: false,
            bun: false,
            brew: false,
            winget: false,
            paru: false,
        }
    }

    #[test]
    fn codex_recommendation_does_not_require_node_or_npm() {
        let source = recommend_for_tool("codex", "macos", "arm64", false, &no_package_managers());

        assert_eq!(source.source, "official");
        let command = source
            .command
            .expect("codex should have standalone installer");
        assert!(command.contains("chatgpt.com/codex/install.sh"));
        assert!(command.contains("bash $tmp"));
        assert!(!command.contains("npm"));
    }

    #[test]
    fn gemini_without_node_or_brew_is_manual_only() {
        let source = recommend_for_tool("gemini", "macos", "arm64", false, &no_package_managers());

        assert_eq!(source.source, "manual");
        assert!(source.command.is_none());
        assert!(source
            .warning
            .as_deref()
            .unwrap_or_default()
            .contains("requires npm/npx or Homebrew"));
    }

    #[test]
    fn gemini_uses_npm_only_when_npm_is_detected() {
        let mut managers = no_package_managers();
        managers.npm = true;

        let source = recommend_for_tool("gemini", "macos", "arm64", false, &managers);

        assert_eq!(source.source, "official");
        assert_eq!(
            source.command.as_deref(),
            Some("npm install -g @google/gemini-cli")
        );
    }

    #[test]
    fn opencode_windows_without_npm_is_manual_only() {
        let source =
            recommend_for_tool("opencode", "windows", "x64", false, &no_package_managers());

        assert_eq!(source.source, "manual");
        assert!(source.command.is_none());
    }

    #[test]
    fn restricted_network_prefers_builtin_oss_source_with_checksum() {
        let oss = [crate::commands::install_sources::BuiltinOssSource {
            tool: "codex_cli",
            platform: "macos",
            arch: "arm64",
            url: "https://oss.example.com/codex/install.sh",
            sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            version: "latest",
            official_url: "https://chatgpt.com/codex/install.sh",
        }];

        let source = recommend_for_tool_with_oss_sources(
            "codex",
            "macos",
            "arm64",
            true,
            &no_package_managers(),
            &oss,
        );

        assert_eq!(source.source, "oss_configured");
        assert_eq!(
            source.url.as_deref(),
            Some("https://oss.example.com/codex/install.sh")
        );
        assert!(source.checksum.is_some());
        assert!(source.command.is_none());
    }

    #[test]
    fn builtin_oss_source_without_checksum_is_ignored() {
        let oss = [crate::commands::install_sources::BuiltinOssSource {
            tool: "codex_cli",
            platform: "macos",
            arch: "arm64",
            url: "https://oss.example.com/codex/install.sh",
            sha256: "",
            version: "latest",
            official_url: "https://chatgpt.com/codex/install.sh",
        }];

        let source = recommend_for_tool_with_oss_sources(
            "codex",
            "macos",
            "arm64",
            true,
            &no_package_managers(),
            &oss,
        );

        assert_eq!(source.source, "official");
        assert!(source.command.is_some());
    }

    #[test]
    fn extracts_first_install_url_from_shell_command() {
        let command =
            "bash -c 'tmp=$(mktemp) && curl -fsSL https://chatgpt.com/codex/install.sh -o $tmp'";

        assert_eq!(
            install_script_url_from_command(Some(command)).as_deref(),
            Some("https://chatgpt.com/codex/install.sh")
        );
    }

    #[test]
    fn install_result_captures_output_tail() {
        let before = EnvironmentToolStatus {
            tool: "codex".to_string(),
            installed: false,
            version: None,
            installed_but_broken: false,
            env_type: "macos".to_string(),
            wsl_distro: None,
            error: Some("missing".to_string()),
        };
        let after = EnvironmentToolStatus {
            tool: "codex".to_string(),
            installed: true,
            version: Some("1.0.0".to_string()),
            installed_but_broken: false,
            env_type: "macos".to_string(),
            wsl_distro: None,
            error: None,
        };
        let output = Command::new("sh")
            .arg("-c")
            .arg("printf 'a\\nb\\nc\\n'")
            .output()
            .expect("test shell output");

        let result = build_install_result(
            "codex",
            false,
            Some("echo install".to_string()),
            before,
            after,
            Ok(output),
        );

        assert!(result.error.is_none());
        assert_eq!(result.stdout_tail.as_deref(), Some("a\nb\nc"));
        assert_eq!(result.after.version.as_deref(), Some("1.0.0"));
    }

    #[tokio::test]
    async fn source_test_for_manual_source_is_not_ok() {
        let source = recommend_for_tool("gemini", "macos", "arm64", false, &no_package_managers());
        let result = test_recommended_source(source)
            .await
            .expect("manual source test should return structured result");

        assert!(!result.ok);
        assert_eq!(result.source, "manual");
        assert!(result.error.is_some());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn configure_claude_writes_replaceable_marked_shell_block() {
        let _guard = env_lock().lock().unwrap();
        let home = unique_temp_home("claude");
        std::env::set_var("CC_SWITCH_TEST_HOME", &home);
        std::env::set_var("SHELL", "/bin/zsh");

        configure_tool("claude", "https://gateway.example.com/", "key-one")
            .expect("first claude config write should succeed");
        configure_tool("claude", "https://gateway.example.com/", "key-two")
            .expect("second claude config write should succeed");

        let content = std::fs::read_to_string(home.join(".zshrc")).expect("read zshrc");
        assert_eq!(content.matches("# >>> TAKO_SWITCH_CLAUDE").count(), 1);
        assert!(content.contains("ANTHROPIC_BASE_URL=\"https://gateway.example.com/api\""));
        assert!(content.contains("ANTHROPIC_AUTH_TOKEN=\"key-two\""));
        assert!(!content.contains("key-one"));

        std::env::remove_var("SHELL");
        std::env::remove_var("CC_SWITCH_TEST_HOME");
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn configure_codex_writes_real_config_files() {
        let _guard = env_lock().lock().unwrap();
        let home = unique_temp_home("codex");
        std::env::set_var("CC_SWITCH_TEST_HOME", &home);

        configure_tool("codex", "https://gateway.example.com/", "codex-key")
            .expect("codex config write should succeed");

        let auth =
            std::fs::read_to_string(home.join(".codex").join("auth.json")).expect("read auth");
        let config =
            std::fs::read_to_string(home.join(".codex").join("config.toml")).expect("read config");
        assert!(auth.contains("\"OPENAI_API_KEY\""));
        assert!(auth.contains("codex-key"));
        assert!(config.contains("model_provider = \"tako\""));
        assert!(config.contains("base_url = \"https://gateway.example.com/v1\""));

        std::env::remove_var("CC_SWITCH_TEST_HOME");
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn configure_gemini_writes_real_env_file_and_preserves_existing_keys() {
        let _guard = env_lock().lock().unwrap();
        let home = unique_temp_home("gemini");
        let gemini_dir = home.join(".gemini");
        std::fs::create_dir_all(&gemini_dir).expect("create gemini dir");
        std::fs::write(gemini_dir.join(".env"), "EXISTING=value\n").expect("seed env");
        std::env::set_var("CC_SWITCH_TEST_HOME", &home);

        configure_tool("gemini", "https://gateway.example.com/", "gemini-key")
            .expect("gemini config write should succeed");

        let content = std::fs::read_to_string(gemini_dir.join(".env")).expect("read gemini env");
        assert!(content.contains("EXISTING=value"));
        assert!(content.contains("GEMINI_API_KEY=gemini-key"));
        assert!(content.contains("GOOGLE_GEMINI_BASE_URL=https://gateway.example.com"));

        std::env::remove_var("CC_SWITCH_TEST_HOME");
        let _ = std::fs::remove_dir_all(home);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn marked_block_replaces_existing_block_without_duplication() {
        let existing = "export PATH=/x\n\n# >>> TAKO_SWITCH_CLAUDE\nold\n# <<< TAKO_SWITCH_CLAUDE\n\nalias ll='ls -l'\n";
        let next = replace_marked_block(
            existing,
            "# >>> TAKO_SWITCH_CLAUDE",
            "# <<< TAKO_SWITCH_CLAUDE",
            "# >>> TAKO_SWITCH_CLAUDE\nnew\n# <<< TAKO_SWITCH_CLAUDE",
        );

        assert!(next.contains("export PATH=/x"));
        assert!(next.contains("new"));
        assert!(!next.contains("\nold\n"));
        assert_eq!(next.matches("# >>> TAKO_SWITCH_CLAUDE").count(), 1);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn shell_escape_double_quoted_escapes_expansion_chars() {
        assert_eq!(
            shell_escape_double_quoted("a\"b$c`d\\e"),
            "a\\\"b\\$c\\`d\\\\e"
        );
    }

    #[test]
    fn sha256_hex_matches_known_value() {
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn downloaded_installer_command_quotes_path() {
        let path = PathBuf::from("/tmp/tako install.sh");
        assert_eq!(
            shell_command_for_downloaded_installer(&path).unwrap(),
            "bash '/tmp/tako install.sh'"
        );
    }

    #[tokio::test]
    #[ignore = "real network test: downloads official installer scripts"]
    async fn real_official_installer_scripts_are_downloadable() {
        let cases = [
            ("claude", "https://claude.ai/install.sh"),
            ("codex", "https://chatgpt.com/codex/install.sh"),
            ("opencode", "https://opencode.ai/install"),
            (
                "hermes",
                "https://raw.githubusercontent.com/NousResearch/hermes-agent/main/scripts/install.sh",
            ),
        ];

        let client = crate::proxy::http_client::get();
        let mut failures = Vec::new();
        for (name, url) in cases {
            let response =
                match tokio::time::timeout(Duration::from_secs(30), client.get(url).send()).await {
                    Ok(Ok(response)) => response,
                    Ok(Err(err)) => {
                        failures.push(format!("{name}: request failed for {url}: {err}"));
                        continue;
                    }
                    Err(_) => {
                        failures.push(format!("{name}: request timed out for {url}"));
                        continue;
                    }
                };

            let status = response.status();
            if !status.is_success() {
                failures.push(format!("{name}: {url} returned HTTP {status}"));
                continue;
            }

            match response.bytes().await {
                Ok(bytes) if installer_body_looks_valid(&bytes) => {}
                Ok(bytes) => failures.push(format!(
                    "{name}: {url} did not return an installer script: {} bytes",
                    bytes.len()
                )),
                Err(err) => failures.push(format!("{name}: body failed for {url}: {err}")),
            }
        }

        assert!(
            failures.is_empty(),
            "real installer download failures:\n{}",
            failures.join("\n")
        );
    }

    fn installer_body_looks_valid(bytes: &[u8]) -> bool {
        if bytes.len() <= 100 {
            return false;
        }
        let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_ascii_lowercase();
        if prefix.contains("<html") || prefix.contains("<!doctype") {
            return false;
        }
        prefix.contains("#!/") || prefix.contains("set -e") || prefix.contains("powershell")
    }
}
