//! API for the X19 standalone miner (`omo-minerd`): live status, persistent
//! configuration, operating presets, logs and boot control.

use axum::{
    Json, Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
    middleware::Next,
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
use std::sync::Mutex;
use std::time::Duration;

/// `user:<hash>`, written by the image build (default) and replaced by the
/// user from the web UI (stored on /data, so it survives reboots and firmware
/// updates). `<hash>` is `pbkdf2-sha256$<iterations>$<salt hex>$<key hex>`;
/// a bare 64-digit SHA-256 hex digest is still accepted for older images.
pub const WEB_AUTH_PATH: &str = "/etc/openmineros/web-auth";
pub const USER_AUTH_PATH: &str = "/data/omo/web-auth";
pub const USER_SHADOW_PATH: &str = "/data/omo/shadow";
const LOG_PATH: &str = "/tmp/omo-minerd.log";
const PBKDF2_ITERATIONS: u32 = 20_000;
/// Delay before answering a failed login, to slow down password guessing.
const FAILED_LOGIN_DELAY: Duration = Duration::from_secs(1);

pub fn routes() -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/v1/x19/status", get(status))
        .route("/api/v1/x19/config", get(get_config).put(put_config))
        .route("/api/v1/x19/presets", get(get_presets))
        .route("/api/v1/x19/log", get(log))
        .route("/api/v1/x19/history", get(history))
        .route("/api/v1/x19/best-history", get(best_history))
        .route("/api/v1/x19/restart", post(restart))
        .route("/api/v1/x19/reboot", post(reboot))
        .route("/api/v1/x19/boot", get(boot_state).post(boot))
        .route(
            "/api/v1/x19/password",
            get(password_state).post(set_password),
        )
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

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Compares without an early exit, so timing does not reveal how many leading
/// bytes matched.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hmac_sha256(key: &[u8], msg: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    for m in msg {
        inner.update(m);
    }
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// PBKDF2-HMAC-SHA256 with a single 32-byte output block (RFC 8018).
fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut u = hmac_sha256(password, &[salt, &1u32.to_be_bytes()]);
    let mut out = u;
    for _ in 1..iterations {
        u = hmac_sha256(password, &[&u]);
        out.iter_mut().zip(u).for_each(|(o, x)| *o ^= x);
    }
    out
}

fn random_salt() -> Result<[u8; 16], String> {
    use std::io::Read;
    let mut salt = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut salt))
        .map_err(|e| format!("random salt: {e}"))?;
    Ok(salt)
}

fn hash_web_password(pw: &str) -> Result<String, String> {
    let salt = random_salt()?;
    let key = pbkdf2_sha256(pw.as_bytes(), &salt, PBKDF2_ITERATIONS);
    Ok(format!(
        "pbkdf2-sha256${PBKDF2_ITERATIONS}${}${}",
        hex(&salt),
        hex(&key)
    ))
}

fn verify_web_password(pw: &str, stored: &str) -> bool {
    let parts: Vec<&str> = stored.split('$').collect();
    match parts.as_slice() {
        ["pbkdf2-sha256", iters, salt, key] => {
            let (Ok(iters), Some(salt), Some(key)) =
                (iters.parse::<u32>(), unhex(salt), unhex(key))
            else {
                return false;
            };
            if !(1..=10_000_000).contains(&iters) {
                return false;
            }
            ct_eq(&pbkdf2_sha256(pw.as_bytes(), &salt, iters), &key)
        }
        // Legacy image default: unsalted SHA-256.
        [legacy] => unhex(legacy)
            .is_some_and(|h| h.len() == 32 && ct_eq(&Sha256::digest(pw.as_bytes()), &h)),
        _ => false,
    }
}

/// Credentials that verified last time, keyed by a digest of the stored auth
/// line and the Authorization header. PBKDF2 is slow on the Zynq on purpose,
/// so the UI's status polling must not pay for it on every request.
static VERIFIED: Mutex<Option<[u8; 32]>> = Mutex::new(None);

fn cache_key(stored: &str, header: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(stored.as_bytes());
    h.update([0]);
    h.update(header.as_bytes());
    h.finalize().into()
}

/// Checks HTTP Basic credentials against USER_AUTH_PATH (falling back to the
/// image default WEB_AUTH_PATH). Without either file (development builds)
/// the API is open.
fn authorized(headers: &HeaderMap) -> bool {
    let Ok(expected) =
        std::fs::read_to_string(USER_AUTH_PATH).or_else(|_| std::fs::read_to_string(WEB_AUTH_PATH))
    else {
        return true;
    };
    let expected = expected.trim();
    let Some((user, hash)) = expected.split_once(':') else {
        return false;
    };
    let Some(raw) = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
    else {
        return false;
    };
    let key = cache_key(expected, raw);
    if let Ok(cached) = VERIFIED.lock() {
        if cached.is_some_and(|c| ct_eq(&c, &key)) {
            return true;
        }
    }
    let Some(given) = raw
        .strip_prefix("Basic ")
        .and_then(b64decode)
        .and_then(|b| String::from_utf8(b).ok())
    else {
        return false;
    };
    let Some((u, p)) = given.split_once(':') else {
        return false;
    };
    let ok = ct_eq(u.as_bytes(), user.as_bytes()) && verify_web_password(p, hash);
    if ok {
        if let Ok(mut cached) = VERIFIED.lock() {
            *cached = Some(key);
        }
    }
    ok
}

/// Blocks cross-site request forgery: a state-changing request must be JSON
/// (which a foreign page cannot send without a CORS preflight this server
/// never grants) and, when the browser names its origin, come from this host.
fn same_origin_write(req: &Request<Body>) -> bool {
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    let h = req.headers();
    let json = h
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.trim_start()
                .to_ascii_lowercase()
                .starts_with("application/json")
        });
    let origin_ok = match h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        None => true,
        Some(origin) => {
            let host = h.get(header::HOST).and_then(|v| v.to_str().ok());
            let origin_host = origin.split_once("://").map(|(_, rest)| rest);
            host.is_some() && origin_host == host
        }
    };
    json && origin_ok
}

/// Authentication and CSRF guard for the whole HTTP API.
pub async fn guard(req: Request<Body>, next: Next) -> Response {
    // PBKDF2 takes a while on the Zynq: keep it off the async workers.
    let headers = req.headers().clone();
    let ok = tokio::task::spawn_blocking(move || authorized(&headers))
        .await
        .unwrap_or(false);
    if !ok {
        tokio::time::sleep(FAILED_LOGIN_DELAY).await;
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"OpenMinerOS\"")],
            "login required",
        )
            .into_response();
    }
    if !same_origin_write(&req) {
        return err(StatusCode::FORBIDDEN, "cross-site request refused");
    }
    next.run(req).await
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

/// Seconds between history samples, and how many are kept (24 h).
const SAMPLE_EVERY: Duration = Duration::from_secs(60);
const SAMPLES_KEPT: usize = 24 * 60;
const BEST_HISTORY_PATH: &str = "/data/omo/best-history.json";

struct Sample {
    at: std::time::Instant,
    ths: f64,
    temps: Vec<Option<f64>>,
    fan: Option<f64>,
    watts: f64,
}

static SAMPLES: Mutex<std::collections::VecDeque<Sample>> =
    Mutex::new(std::collections::VecDeque::new());

/// One history point from the miner's status file. A status older than two
/// minutes means the miner is not running: zero hashrate, idle power.
fn sample_from_status(v: &Value, now: u64) -> (f64, Vec<Option<f64>>, Option<f64>, f64) {
    let fresh = v["updated"]
        .as_u64()
        .is_some_and(|u| now.saturating_sub(u) <= 120);
    let mining = fresh && v["state"].as_str() == Some("mining");
    if !fresh {
        return (0.0, vec![], None, 60.0);
    }
    let temps = v["temps_pcb"]
        .as_array()
        .map(|a| a.iter().map(|t| t.as_f64().filter(|t| *t > 0.0)).collect())
        .unwrap_or_default();
    let watts = match (v["freq_mhz"].as_f64(), v["voltage"].as_f64()) {
        (Some(f), Some(u)) if mining => estimated_power(f, u).round(),
        _ => 60.0,
    };
    (
        if mining {
            v["hashrate_ths"].as_f64().unwrap_or(0.0)
        } else {
            0.0
        },
        temps,
        v["fan_percent"].as_f64(),
        watts,
    )
}

/// Samples the miner status once a minute into an in-memory 24 h ring. It is
/// kept in RAM on purpose: a write per minute would wear the NAND.
pub fn spawn_sampler() {
    tokio::spawn(async {
        let mut tick = tokio::time::interval(SAMPLE_EVERY);
        loop {
            tick.tick().await;
            let v: Value = std::fs::read_to_string(STATUS_PATH)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or(Value::Null);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let (ths, temps, fan, watts) = sample_from_status(&v, now);
            if let Ok(mut q) = SAMPLES.lock() {
                if q.len() >= SAMPLES_KEPT {
                    q.pop_front();
                }
                q.push_back(Sample {
                    at: std::time::Instant::now(),
                    ths,
                    temps,
                    fan,
                    watts,
                });
            }
        }
    });
}

/// Last 24 h, oldest first. Points carry their age in seconds because the
/// board clock is not set; the browser turns ages into local times.
async fn history() -> Json<Value> {
    let samples = SAMPLES.lock().map(|q| {
        q.iter()
            .map(|s| {
                json!({
                    "age_s": s.at.elapsed().as_secs(),
                    "ths": (s.ths * 100.0).round() / 100.0,
                    "temps": s.temps,
                    "fan": s.fan,
                    "w": s.watts,
                })
            })
            .collect::<Vec<_>>()
    });
    Json(json!({
        "interval_s": SAMPLE_EVERY.as_secs(),
        "samples": samples.unwrap_or_default(),
    }))
}

/// Best share per day (`[[unix_day, difficulty], ...]`), dated by the pool.
async fn best_history() -> Json<Value> {
    Json(
        std::fs::read_to_string(BEST_HISTORY_PATH)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!({ "days": [] })),
    )
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

/// Reboots after a delay, so the HTTP answer still reaches the browser.
fn reboot_later() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(10));
        let _ = Command::new("reboot").status();
    });
}

async fn reboot() -> Json<Value> {
    restart_miner();
    reboot_later();
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
            reboot_later();
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
    let auth = match hash_web_password(&pw) {
        Ok(h) => format!("root:{h}\n"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
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

    #[test]
    fn history_sample_reads_status() {
        let v = json!({
            "state": "mining", "updated": 1000, "hashrate_ths": 14.6,
            "temps_pcb": [41.5, 0.0, 43.0], "fan_percent": 35,
            "freq_mhz": 109.0, "voltage": 11.96,
        });
        let (ths, temps, fan, w) = sample_from_status(&v, 1030);
        assert_eq!(ths, 14.6);
        assert_eq!(temps, vec![Some(41.5), None, Some(43.0)]);
        assert_eq!(fan, Some(35.0));
        assert_eq!(w, estimated_power(109.0, 11.96).round());
        let (ths, temps, _, w) = sample_from_status(&v, 2000);
        assert_eq!((ths, temps.len(), w), (0.0, 0, 60.0));
    }

    #[test]
    fn pbkdf2_matches_rfc7914_vector() {
        // RFC 7914 section 11: PBKDF2-HMAC-SHA256("passwd", "salt", 1).
        assert_eq!(
            hex(&pbkdf2_sha256(b"passwd", b"salt", 1)),
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc"
        );
    }

    #[test]
    fn web_password_round_trips_and_accepts_legacy() {
        let salt = [7u8; 16];
        let stored = format!(
            "pbkdf2-sha256$100${}${}",
            hex(&salt),
            hex(&pbkdf2_sha256(b"secret12", &salt, 100))
        );
        assert!(verify_web_password("secret12", &stored));
        assert!(!verify_web_password("secret13", &stored));
        let legacy = hex(&Sha256::digest(b"admin"));
        assert!(verify_web_password("admin", &legacy));
        assert!(!verify_web_password("admin2", &legacy));
        assert!(!verify_web_password("x", "garbage"));
    }

    fn req(method: Method, headers: &[(header::HeaderName, &str)]) -> Request<Body> {
        let mut r = Request::builder().method(method);
        for (k, v) in headers {
            r = r.header(k, *v);
        }
        r.body(Body::empty()).unwrap()
    }

    #[test]
    fn csrf_guard_blocks_foreign_and_non_json_writes() {
        let json = (header::CONTENT_TYPE, "application/json");
        let host = (header::HOST, "10.0.0.5");
        assert!(same_origin_write(&req(Method::GET, &[])));
        assert!(same_origin_write(&req(
            Method::POST,
            &[json.clone(), host.clone()]
        )));
        assert!(same_origin_write(&req(
            Method::POST,
            &[
                json.clone(),
                host.clone(),
                (header::ORIGIN, "http://10.0.0.5")
            ]
        )));
        assert!(!same_origin_write(&req(
            Method::POST,
            &[
                json.clone(),
                host.clone(),
                (header::ORIGIN, "https://evil.example")
            ]
        )));
        assert!(!same_origin_write(&req(
            Method::POST,
            &[(header::CONTENT_TYPE, "text/plain"), host]
        )));
    }
}
