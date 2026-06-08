#![allow(non_snake_case)]

//! Tako remote control — drives the local `tako-remote` (Happy) CLI daemon so
//! a user can hand off the current coding session to phone/browser via the
//! self-hosted Tako backend. This is the cc-switch ↔ Happy fusion seam.
//!
//! #47 — auth handshake is复刻 from happy CLI (`packages/happy-cli/src/ui/auth.ts`):
//! generate an ephemeral NaCl `box` keypair → `POST {server}/v1/auth/request`
//! (publicKey + supportsV2) → web URL `{webapp}/terminal/connect#key=<b64url(pk)>`
//! → poll the same endpoint until `state == "authorized"` → decrypt the response
//! bundle (ephPub[32]+nonce[24]+ct) with our secret → write `access.key` so the
//! daemon picks up the credentials. Additive — no upstream files changed.

use base64::Engine;
use crypto_box::{
    aead::{Aead},
    PublicKey, SalsaBox, SecretKey,
};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use std::process::Stdio;
use tauri::State;
use tokio::process::Command;

use crate::commands::migration::current_tako_key;
use crate::store::AppState;

const TAKO_SERVER_URL: &str = "https://happy.shiroha.tech";
const TAKO_WEBAPP_URL: &str = "https://happy-remote.shiroha.tech";
const REMOTE_BIN: &str = "tako-remote";
const INSTALL_URL: &str = "https://tako.shiroha.tech/install.sh";
const CLI_VERSION_HEADER: &str = "cli/tako-switch";

/// NaCl box nonce length (XSalsa20Poly1305) = 24 bytes, matching tweetnacl.
const NONCE_LEN: usize = 24;

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// base64url without padding, matching happy's `encodeBase64Url`.
fn b64url_nopad(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Resolve the daemon home dir the same way the fork's `configuration.ts` does:
/// honor `HAPPY_HOME_DIR` (expanding a leading `~`), else `~/.happy`.
fn happy_home_dir() -> std::path::PathBuf {
    if let Ok(custom) = std::env::var("HAPPY_HOME_DIR") {
        let expanded = if let Some(rest) = custom.strip_prefix('~') {
            crate::config::get_home_dir().join(rest.trim_start_matches('/'))
        } else {
            std::path::PathBuf::from(custom)
        };
        return expanded;
    }
    crate::config::get_home_dir().join(".happy")
}

#[derive(serde::Serialize)]
pub struct RemoteStatus {
    pub installed: bool,
    pub running: bool,
    pub version: Option<String>,
}

fn no_window(cmd: &mut Command) {
    let _ = cmd;
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
}

/// Is the tako-remote CLI installed? Detect by locating the binary on PATH and
/// reading the package.json version — WITHOUT executing it. Running the CLI
/// (even `--version`) makes the happy fork drop into an interactive Ink auth
/// prompt when unauthenticated, which crashes with "Raw mode is not supported"
/// and pollutes stdout. Never spawn tako-remote from a detection path.
#[tauri::command]
pub async fn remote_status() -> Result<RemoteStatus, String> {
    let bin = which_remote_bin();
    let installed = bin.is_some();
    let version = bin.as_ref().and_then(|p| remote_version_from_pkg(p));
    let running = if installed { is_daemon_running().await } else { false };
    log::info!(
        "[remote] status: installed={installed} running={running} version={version:?} bin={bin:?}"
    );
    Ok(RemoteStatus { installed, running, version })
}

/// Locate the `tako-remote` executable on PATH (like `which`), without running it.
fn which_remote_bin() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(REMOTE_BIN);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Read the installed package version from the npm global package.json,
/// resolving the bin symlink to find the package root. No CLI execution.
fn remote_version_from_pkg(bin: &std::path::Path) -> Option<String> {
    let resolved = std::fs::canonicalize(bin).ok()?;
    let mut dir = resolved.parent();
    while let Some(d) = dir {
        let pkg = d.join("package.json");
        if pkg.exists() {
            let text = std::fs::read_to_string(&pkg).ok()?;
            let json: serde_json::Value = serde_json::from_str(&text).ok()?;
            if json.get("name").and_then(|n| n.as_str()) == Some("tako-remote") {
                return json.get("version").and_then(|v| v.as_str()).map(String::from);
            }
        }
        dir = d.parent();
    }
    None
}

async fn is_daemon_running() -> bool {
    let state_file = happy_home_dir().join("daemon.state.json");
    let Ok(text) = std::fs::read_to_string(&state_file) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Some(pid) = json.get("pid").and_then(|p| p.as_i64()) else {
        return false;
    };
    pid_alive(pid)
}

#[cfg(unix)]
fn pid_alive(pid: i64) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn pid_alive(_pid: i64) -> bool {
    false
}

/// Derive a STABLE 32-byte happy account secret from the Tako cr_ key, so one
/// Tako user always maps to one happy account (global login state). The account
/// identity = Ed25519 keypair seeded by this secret; the same bytes double as
/// the legacy E2E secret (matching happy's account-secret model).
fn derive_account_secret(tako_key: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"tako-switch/happy-account/v1");
    hasher.update(tako_key.as_bytes());
    hasher.finalize().into()
}

/// Register (or refresh) the happy account for this Tako user via `/v1/auth`,
/// returning the bearer token. Ed25519 work is confined to a sync scope so no
/// signing types are held across the await.
async fn register_account(
    client: &reqwest::Client,
    tako_key: &str,
    account_secret: &[u8; 32],
) -> Result<String, String> {
    let (public_b64, challenge_b64, signature_b64) = {
        let signing = SigningKey::from_bytes(account_secret);
        let verifying = signing.verifying_key();
        let challenge: [u8; 32] = rand::random();
        let signature = signing.sign(&challenge);
        (
            b64().encode(verifying.to_bytes()),
            b64().encode(challenge),
            b64().encode(signature.to_bytes()),
        )
    };

    let resp: serde_json::Value = client
        .post(format!("{TAKO_SERVER_URL}/v1/auth"))
        .header("X-Happy-Client", CLI_VERSION_HEADER)
        .header("X-Tako-Key", tako_key)
        .json(&serde_json::json!({
            "publicKey": public_b64,
            "challenge": challenge_b64,
            "signature": signature_b64,
            "takoKey": tako_key,
        }))
        .send()
        .await
        .map_err(|e| format!("Account registration failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Tako membership rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Bad /v1/auth response: {e}"))?;

    resp.get("token")
        .and_then(|t| t.as_str())
        .map(String::from)
        .ok_or_else(|| "Registration returned no token".to_string())
}

/// Result of starting the auth handshake: the web URL to render as a QR code
/// (and open in a browser), plus the ephemeral keypair the frontend holds and
/// passes back into `remote_auth_poll`. The secret never touches disk until
/// authentication succeeds.
#[derive(serde::Serialize)]
pub struct RemoteAuthBegin {
    pub web_url: String,
    pub public_key_b64: String,
    pub secret_key_b64: String,
}

/// Step 1 — start the web-auth handshake (复刻 happy `doAuth`/`doWebAuth`).
/// Generates an ephemeral NaCl box keypair, registers the public key with the
/// server, and returns the URL the user scans/opens to authorize. The active
/// Tako cr_ key (read from the built-in provider) gates membership via
/// `X-Tako-Key` — so the user must be logged in first.
#[tauri::command]
pub async fn remote_auth_begin(state: State<'_, AppState>) -> Result<RemoteAuthBegin, String> {
    let tako_key = current_tako_key(&state)
        .ok_or("Please log in to Tako first (no cr_ key found)")?;

    let client = reqwest::Client::new();

    // (1) Tako identity -> stable happy account token.
    let account_secret = derive_account_secret(&tako_key);
    log::info!("[remote] auth_begin: registering happy account from Tako key");
    let account_token = register_account(&client, &tako_key, &account_secret).await?;

    // (2) Ephemeral terminal keypair, registered for authorization.
    let secret = SecretKey::generate(&mut rand::thread_rng());
    let public = secret.public_key();
    let public_b64 = b64().encode(public.as_bytes());

    client
        .post(format!("{TAKO_SERVER_URL}/v1/auth/request"))
        .header("X-Happy-Client", CLI_VERSION_HEADER)
        .header("X-Tako-Key", &tako_key)
        .json(&serde_json::json!({ "publicKey": public_b64, "supportsV2": true }))
        .send()
        .await
        .map_err(|e| format!("Failed to create auth request: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Server rejected auth request: {e}"))?;

    // (3) Authorize URL: terminal key + cred ticket (Tako-derived login state),
    // so the webapp opens already logged in as the Tako identity.
    let cred = serde_json::json!({
        "token": account_token,
        "secret": b64url_nopad(&account_secret),
    });
    let web_url = format!(
        "{TAKO_WEBAPP_URL}/terminal/connect#key={}&cred={}",
        b64url_nopad(public.as_bytes()),
        b64url_nopad(cred.to_string().as_bytes()),
    );
    Ok(RemoteAuthBegin {
        web_url,
        public_key_b64: public_b64,
        secret_key_b64: b64().encode(secret.to_bytes()),
    })
}

#[derive(serde::Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum RemoteAuthPoll {
    /// Not yet authorized — frontend should poll again.
    Pending,
    /// Authorized; credentials written to `access.key`. Daemon can now start.
    Authorized,
}

/// Step 2 — poll the server once for authorization (复刻 `waitForAuthentication`
/// without the loop; the frontend drives the interval). On `authorized`,
/// decrypts the response bundle and writes `access.key` in the exact format the
/// daemon reads (legacy `secret` or `dataKey` variant).
#[tauri::command]
pub async fn remote_auth_poll(
    publicKeyB64: String,
    secretKeyB64: String,
) -> Result<RemoteAuthPoll, String> {
    let client = reqwest::Client::new();
    let resp: serde_json::Value = client
        .post(format!("{TAKO_SERVER_URL}/v1/auth/request"))
        .header("X-Happy-Client", CLI_VERSION_HEADER)
        .json(&serde_json::json!({ "publicKey": publicKeyB64, "supportsV2": true }))
        .send()
        .await
        .map_err(|e| format!("Poll failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Bad poll response: {e}"))?;

    if resp.get("state").and_then(|s| s.as_str()) != Some("authorized") {
        return Ok(RemoteAuthPoll::Pending);
    }

    let token = resp
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or("Authorized response missing token")?
        .to_string();
    let response_b64 = resp
        .get("response")
        .and_then(|r| r.as_str())
        .ok_or("Authorized response missing encrypted payload")?;

    let secret_bytes = b64()
        .decode(secretKeyB64.trim())
        .map_err(|e| format!("Bad secret key: {e}"))?;
    let secret = SecretKey::from_slice(&secret_bytes)
        .map_err(|_| "Secret key must be 32 bytes".to_string())?;

    let decrypted = decrypt_bundle(response_b64, &secret)?;
    write_credentials(&decrypted, &token)?;
    Ok(RemoteAuthPoll::Authorized)
}

/// Decrypt the `ephPub[32] + nonce[24] + ciphertext` bundle with our ephemeral
/// secret, matching happy's `decryptWithEphemeralKey` (tweetnacl `box.open`).
fn decrypt_bundle(response_b64: &str, secret: &SecretKey) -> Result<Vec<u8>, String> {
    let bundle = b64()
        .decode(response_b64)
        .map_err(|e| format!("Bad encrypted payload: {e}"))?;
    if bundle.len() < 32 + NONCE_LEN {
        return Err("Encrypted payload too short".into());
    }
    let eph_pub = PublicKey::from_slice(&bundle[0..32])
        .map_err(|_| "Bad ephemeral public key".to_string())?;
    let nonce = crypto_box::Nonce::from_slice(&bundle[32..32 + NONCE_LEN]);
    let ciphertext = &bundle[32 + NONCE_LEN..];

    SalsaBox::new(&eph_pub, secret)
        .decrypt(nonce, ciphertext)
        .map_err(|_| "Failed to decrypt response — please try again".to_string())
}

/// Write `access.key` in the format the daemon reads. `len == 32` → legacy
/// secret; first byte `0` → dataKey (publicKey + locally-generated machineKey).
fn write_credentials(decrypted: &[u8], token: &str) -> Result<(), String> {
    let dir = happy_home_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let path = dir.join("access.key");

    let json = if decrypted.len() == 32 {
        serde_json::json!({ "secret": b64().encode(decrypted), "token": token })
    } else if decrypted.first() == Some(&0) && decrypted.len() >= 33 {
        let public_key = &decrypted[1..33];
        let machine_key: [u8; 32] = rand::random();
        serde_json::json!({
            "encryption": {
                "publicKey": b64().encode(public_key),
                "machineKey": b64().encode(machine_key),
            },
            "token": token,
        })
    } else {
        return Err("Unrecognized credential format from server".into());
    };

    std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap())
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(())
}

/// Start the local daemon once credentials exist (`access.key` written by the
/// auth handshake). Injects the server URL so the daemon talks to our backend.
#[tauri::command]
pub async fn remote_start_daemon() -> Result<bool, String> {
    let mut cmd = Command::new(REMOTE_BIN);
    cmd.args(["daemon", "start"])
        .env("HAPPY_SERVER_URL", TAKO_SERVER_URL)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_window(&mut cmd);

    let out = cmd.output().await.map_err(|e| format!("Failed to start daemon: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Daemon failed to start: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(true)
}

/// Stop the local daemon.
#[tauri::command]
pub async fn remote_stop_daemon() -> Result<bool, String> {
    let mut cmd = Command::new(REMOTE_BIN);
    cmd.args(["daemon", "stop"]).stdout(Stdio::null()).stderr(Stdio::null());
    no_window(&mut cmd);
    let status = cmd.status().await.map_err(|e| format!("Failed to stop daemon: {e}"))?;
    Ok(status.success())
}

/// Trigger the one-click install path (Node auto-install + CLI). macOS/Linux
/// run the shell installer; Windows users are pointed at the download page.
#[tauri::command]
pub async fn remote_install() -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        return Err("On Windows, install tako-remote from the download page.".into());
    }
    #[cfg(not(target_os = "windows"))]
    {
        let script = format!("curl -fsSL {INSTALL_URL} | bash");
        let mut cmd = Command::new("bash");
        cmd.args(["-lc", &script]).stdout(Stdio::piped()).stderr(Stdio::piped());
        no_window(&mut cmd);
        let out = cmd.output().await.map_err(|e| format!("Install failed to launch: {e}"))?;
        if !out.status.success() {
            return Err(format!("Install failed: {}", String::from_utf8_lossy(&out.stderr)));
        }
        Ok(true)
    }
}

// ── Tako statusline ─────────────────────────────────────────────
// Inject/remove Tako's statusline into ~/.claude/settings.json. The statusline
// is rendered by `tako-remote statusline` (dir/git/model/context/quota).

fn claude_settings_path() -> std::path::PathBuf {
    crate::config::get_home_dir().join(".claude").join("settings.json")
}

/// The command Claude Code runs for the statusline. Points at the Tako Switch
/// binary itself (`<exe> statusline`) so it works without the Tako CLI.
fn statusline_command() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "tako-switch".to_string());
    format!("\"{exe}\" statusline")
}

/// Whether the Tako statusline is currently configured in Claude settings.
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

/// Inject the Tako statusline into ~/.claude/settings.json (preserving other keys).
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

/// Remove only the Tako statusline (leave user's other config untouched).
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
