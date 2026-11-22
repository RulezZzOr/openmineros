//! API for the X19 standalone miner (`omo-minerd`): live status, persistent
//! configuration, operating presets, logs and boot control.

use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use openmineros_common::miner_config::{
    CONFIG_PATH, MinerConfig, STATUS_PATH, estimated_power, presets,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::process::Command;

/// `user:sha256hex(password)`, written by the image build (default) and
/// replaced by the user from the web UI (stored on /data, so it survives
/// reboots and firmware updates).
pub const WEB_AUTH_PATH: &str = "/etc/openmineros/web-auth";
pub const USER_AUTH_PATH: &str = "/data/omo/web-auth";
pub const USER_SHADOW_PATH: &str = "/data/omo/shadow";
const LOG_PATH: &str = "/tmp/omo-minerd.log";

pub fn routes() -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/v1/x19/status", get(status))
        .route("/api/v1/x19/config", get(get_config).put(put_config))
        .route("/api/v1/x19/presets", get(get_presets))
        .route("/api/v1/x19/log", get(log))
        .route("/api/v1/x19/restart", post(restart))
        .route("/api/v1/x19/reboot", post(reboot))
        .route("/api/v1/x19/boot", get(boot_state).post(boot))
        .route(
            "/api/v1/x19/password",
            get(password_state).post(set_password),
        )
        .layer(middleware::from_fn(basic_auth))
}

async fn index() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../../../web/index.html"))
}

fn b64decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Checks HTTP Basic credentials against WEB_AUTH_PATH. Without that file
/// (development builds) the API is open.
fn authorized(req: &Request<Body>) -> bool {
    let Ok(expected) =
        std::fs::read_to_string(USER_AUTH_PATH).or_else(|_| std::fs::read_to_string(WEB_AUTH_PATH))
    else {
        return true;
    };
    let Some((user, hash)) = expected.trim().split_once(':') else {
        return false;
    };
    let Some(given) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Basic "))
        .and_then(b64decode)
        .and_then(|b| String::from_utf8(b).ok())
    else {
        return false;
    };
    let Some((u, p)) = given.split_once(':') else {
        return false;
    };
    u == user && hex(&Sha256::digest(p.as_bytes())) == hash
}

async fn basic_auth(req: Request<Body>, next: Next) -> Response {
    if authorized(&req) {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"OpenMinerOS\"")],
            "login required",
        )
            .into_response()
    }
}

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

async fn status() -> Json<Value> {
    let mut v: Value = std::fs::read_to_string(STATUS_PATH)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({ "state": "unknown", "message": "miner has not reported yet" }));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Some(o) = v.as_object_mut() {
        o.insert("now".into(), json!(now));
        let f = o.get("freq_mhz").and_then(Value::as_f64);
        let v = o.get("voltage").and_then(Value::as_f64);
        if let (Some(f), Some(v)) = (f, v) {
            let mining = o.get("state").and_then(Value::as_str) == Some("mining");
            o.insert(
                "est_power_w".into(),
                json!(if mining {
                    estimated_power(f, v).round()
                } else {
                    60.0
                }),
            );
        }
        if let Ok(up) = std::fs::read_to_string("/proc/uptime") {
            o.insert(
                "system_uptime_s".into(),
                json!(up.split('.').next().and_then(|x| x.parse::<u64>().ok())),
            );
        }
    }
    Json(v)
}

async fn get_config() -> Response {
    match std::fs::read_to_string(CONFIG_PATH)
        .ok()
        .and_then(|s| serde_json::from_str::<MinerConfig>(&s).ok())
    {
        Some(c) => Json(c).into_response(),
        None => Json(MinerConfig::default()).into_response(),
    }
}

fn restart_miner() {
    let _ = Command::new("killall")
        .args(["-TERM", "omo-minerd"])
        .status();
}

async fn put_config(Json(cfg): Json<MinerConfig>) -> Response {
    if let Err(e) = cfg.save(CONFIG_PATH) {
        return err(StatusCode::BAD_REQUEST, e);
    }
    restart_miner();
    Json(json!({ "saved": true, "restarting": true })).into_response()
}

async fn get_presets() -> Json<Value> {
    Json(Value::Array(
        presets()
            .into_iter()
            .map(|(name, f, v, note)| {
                json!({ "name": name, "freq_mhz": f, "voltage": v, "note": note, "est_power_w": estimated_power(f, v).round() })
            })
            .collect(),
    ))
}

async fn log() -> String {
    let text = std::fs::read_to_string(LOG_PATH).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(200)..].join("\n")
}

async fn restart() -> Json<Value> {
    restart_miner();
    Json(json!({ "restarting": true }))
}

async fn reboot() -> Json<Value> {
    restart_miner();
    let _ = Command::new("sh").args(["-c", "sleep 10; reboot"]).spawn();
    Json(json!({ "rebooting": true }))
}

fn bootenv(args: &[&str]) -> Result<String, String> {
    let out = Command::new("/usr/bin/omo-bootenv")
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

async fn boot_state() -> Response {
    match bootenv(&["get", "firmware", "upgrade_stage"]) {
        Ok(t) => {
            let get = |k: &str| {
                t.lines()
                    .find_map(|l| l.strip_prefix(&format!("{k}=")))
                    .unwrap_or("")
                    .to_owned()
            };
            let stage = get("upgrade_stage");
            Json(json!({
                "slot": get("firmware"),
                "trial": !stage.is_empty(),
                "upgrade_stage": stage,
            }))
            .into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Deserialize)]
struct BootReq {
    /// "keep" = make OpenMinerOS permanent; "braiins" = next boot Braiins OS.
    target: String,
}

async fn boot(Json(r): Json<BootReq>) -> Response {
    let res = match r.target.as_str() {
        "keep" => bootenv(&["keep-openmineros"]),
        "braiins" => bootenv(&["boot-braiins"]).inspect(|_| {
            restart_miner();
            let _ = Command::new("sh").args(["-c", "sleep 10; reboot"]).spawn();
        }),
        _ => return err(StatusCode::BAD_REQUEST, "target must be keep or braiins"),
    };
    match res {
        Ok(t) => Json(json!({ "ok": true, "message": t.trim() })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn password_state() -> Json<Value> {
    Json(json!({ "custom": std::path::Path::new(USER_AUTH_PATH).exists() }))
}

#[derive(Deserialize)]
struct PasswordReq {
    password: String,
}

/// Sets the root password for the web UI and telnet. The telnet hash goes to
/// /data/omo/shadow, bind-mounted over the read-only /etc/shadow at boot.
async fn set_password(Json(r): Json<PasswordReq>) -> Response {
    let pw = r.password;
    if pw.chars().count() < 8 || pw.contains(['\n', ':']) {
        return err(
            StatusCode::BAD_REQUEST,
            "password needs at least 8 characters and no ':' or newline",
        );
    }
    let hash = match hash_for_shadow(&pw) {
        Ok(h) => h,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    if let Err(e) = std::fs::create_dir_all("/data/omo") {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    // Write the shadow file in place: /etc/shadow may be a bind mount of it.
    let shadow = format!("root:{hash}:19000:0:99999:7:::\n");
    if let Err(e) = std::fs::write(USER_SHADOW_PATH, &shadow) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let mounted = std::fs::read_to_string("/proc/mounts")
        .map(|m| m.contains(" /etc/shadow "))
        .unwrap_or(false);
    if !mounted {
        let _ = Command::new("mount")
            .args(["--bind", USER_SHADOW_PATH, "/etc/shadow"])
            .status();
    }
    let auth = format!("root:{}\n", hex(&Sha256::digest(pw.as_bytes())));
    let tmp = format!("{USER_AUTH_PATH}.tmp");
    if let Err(e) = std::fs::write(&tmp, auth).and_then(|_| std::fs::rename(&tmp, USER_AUTH_PATH)) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    Json(json!({ "ok": true, "message": "Password changed. Log in again with the new password." }))
        .into_response()
}

fn hash_for_shadow(pw: &str) -> Result<String, String> {
    use std::io::Write;
    let mut child = Command::new("mkpasswd")
        .args(["-m", "sha512", "-P", "0"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("mkpasswd: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("mkpasswd stdin")?
        .write_all(pw.as_bytes())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let h = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if h.starts_with("$6$") {
        Ok(h)
    } else {
        Err("password hashing failed".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_basic_credentials() {
        assert_eq!(b64decode("cm9vdDpzZWNyZXQ=").unwrap(), b"root:secret");
        assert!(b64decode("@@").is_none());
    }
}
