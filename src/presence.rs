//! Presence client for the `eglass-presence` Cloudflare Worker.
//!
//! A forked Eagle Glass build reports "I am watching this" to a shared worker,
//! which keeps a roster in KV. Every client reads that roster back and shows
//! who else is on the same Xtream line.
//!
//! One long-lived background thread does all the network work. It reads the
//! desired state out of an `Arc<Mutex<Shared>>` that the UI writes each frame,
//! and publishes what it learns into an `Arc<Mutex<Roster>>` the UI reads. The
//! UI never blocks on a socket.
//!
//! `ureq` is used for the HTTP, matching `xtream.rs`.

use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The friend group. The worker is the source of truth and the app refreshes
/// this from `GET /api/profiles`, but a hardcoded copy means the dropdown still
/// works on a first run with no network.
pub const DEFAULT_PROFILES: [(&str, &str); 4] = [
    ("peter", "Peter"),
    ("blain", "Blain"),
    ("johnny", "Johnny"),
    ("panda", "Panda"),
];

const HEARTBEAT_MS: i64 = 90_000;
const POLL_ACTIVE_MS: i64 = 20_000;
const POLL_IDLE_MS: i64 = 60_000;
/// How often to re-read `active_cons` / `max_connections` from the panel.
const ACCOUNT_REFRESH_MS: i64 = 90_000;
const TICK_MS: u64 = 500;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A stable per-install id, so two devices belonging to the same friend are
/// counted separately (they consume two connections on the panel).
///
/// Deliberately dependency-free: a xorshift seeded from the clock and the
/// process id is plenty for a key that only has to be unique among four people.
pub fn new_device_id() -> String {
    let mut s = now_ms() as u64 ^ ((std::process::id() as u64) << 32);
    let mut out = String::with_capacity(32);
    for _ in 0..4 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        out.push_str(&format!("{:08x}", (s & 0xffff_ffff) as u32));
    }
    out
}

pub fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this device".to_string())
}

/* ------------------------------------------------------------- shared state */

/// Written by the UI, read by the presence thread.
#[derive(Clone, Default)]
pub struct Shared {
    /// url + token + profile + device id all set, i.e. presence can actually work.
    pub enabled: bool,
    pub url: String,
    pub token: String,
    pub profile: String,
    pub device_id: String,
    pub device: String,

    /// A stream has been started this session. Note this is *not* libVLC's
    /// playing state: a paused stream still holds a panel connection, so
    /// pausing must keep the heartbeat alive.
    pub active: bool,
    /// "playing" | "paused", for display only.
    pub state: String,
    pub stream_kind: String,
    pub stream_id: String,
    pub stream_name: String,
    pub since: i64,

    /// Panel credentials, used only to refresh the connection counters.
    pub server: String,
    pub user: String,
    pub pass: String,
    pub active_cons: i64,
    pub max_connections: i64,

    /// A profile abandoned in the settings dialog: clear its old KV entry so a
    /// name correction does not leave a ghost in the roster until the TTL.
    pub stale_profile: String,
    /// Set by the "Refresh" button to pull the roster on the next tick.
    pub force_poll: bool,
    /// Set when a stream starts, so `active_cons` is re-read from the panel
    /// straight away. Otherwise the reported count is up to a minute stale -
    /// and right after you press play it still says 0, because that reading
    /// predates your own stream.
    pub force_account: bool,

    pub shutdown: bool,
}

impl Shared {
    /// What counts as "a different thing to report". Deliberately excludes
    /// `state`: libVLC reports not-playing while it buffers, so including it made
    /// the signature flap and roughly doubled the heartbeat rate (and therefore
    /// the KV write cost). Pausing is cosmetic - the connection is held either
    /// way - so it can wait for the next timed beat.
    pub fn signature(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.profile, self.stream_kind, self.stream_id, self.stream_name
        )
    }
}

#[derive(Clone, Default, Debug)]
pub struct Watcher {
    pub profile: String,
    pub name: String,
    pub device: String,
    pub state: String,
    pub stream_name: String,
    pub age_seconds: i64,
    pub watching_seconds: i64,
}

/// Published by the presence thread, read by the UI.
#[derive(Clone, Default)]
pub struct Roster {
    pub watchers: Vec<Watcher>,
    pub active_cons: i64,
    pub max_connections: i64,
    pub unaccounted: i64,
    pub fetched_at: i64,
    pub profiles: Vec<(String, String)>,
    pub error: Option<String>,
}

impl Roster {
    pub fn fresh(&self) -> bool {
        self.fetched_at > 0
    }

    /// Everyone in the roster who is not us.
    pub fn others(&self, me: &str) -> Vec<&Watcher> {
        self.watchers.iter().filter(|w| w.profile != me).collect()
    }

    /// Whether we are the ones holding the line. The worker keeps one entry per
    /// person, so this is a flag rather than a count.
    pub fn mine(&self, me: &str) -> bool {
        self.watchers.iter().any(|w| w.profile == me)
    }
}

/* ------------------------------------------------------------------ client */

pub struct Presence {
    pub url: String,
    pub token: String,
}

impl Presence {
    pub fn new(url: &str, token: &str) -> Self {
        Presence {
            url: url.trim().trim_end_matches('/').to_string(),
            token: token.trim().to_string(),
        }
    }

    fn get(&self, path: &str, timeout: u64) -> Result<Value, String> {
        let resp = ureq::builder()
            .timeout(Duration::from_secs(timeout))
            .build()
            .get(&format!("{}{}", self.url, path))
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("User-Agent", "EagleGlassRS/presence")
            .call()
            .map_err(|e| short(&e.to_string()))?;
        let text = resp
            .into_string()
            .map_err(|e| short(&e.to_string()))?;
        serde_json::from_str(&text).map_err(|_| "worker did not return JSON".to_string())
    }

    fn post(&self, path: &str, body: &Value, timeout: u64) -> Result<Value, String> {
        let resp = ureq::builder()
            .timeout(Duration::from_secs(timeout))
            .build()
            .post(&format!("{}{}", self.url, path))
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .set("User-Agent", "EagleGlassRS/presence")
            .send_string(&body.to_string())
            .map_err(|e| short(&e.to_string()))?;
        let text = resp
            .into_string()
            .map_err(|e| short(&e.to_string()))?;
        serde_json::from_str(&text).map_err(|_| "worker did not return JSON".to_string())
    }

    pub fn heartbeat(&self, s: &Shared) -> Result<Roster, String> {
        let body = json!({
            "profile": s.profile,
            "deviceId": s.device_id,
            "device": s.device,
            "state": if s.state.is_empty() { "playing" } else { &s.state },
            "since": s.since,
            "stream": { "kind": s.stream_kind, "id": s.stream_id, "name": s.stream_name },
            "account": { "activeCons": s.active_cons, "maxConnections": s.max_connections },
        });
        self.post("/api/heartbeat", &body, 8).and_then(parse_roster)
    }

    pub fn idle(&self, profile: &str, device_id: &str) -> Result<(), String> {
        let body = json!({ "profile": profile, "deviceId": device_id });
        self.post("/api/idle", &body, 4).map(|_| ())
    }

    pub fn roster(&self) -> Result<Roster, String> {
        self.get("/api/presence", 8).and_then(parse_roster)
    }

    pub fn profiles(&self) -> Result<Vec<(String, String)>, String> {
        let v = self.get("/api/profiles", 8)?;
        Ok(v.get("profiles")
            .and_then(|p| p.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| {
                        let id = e.get("id")?.as_str()?.to_string();
                        let name = e.get("name")?.as_str()?.to_string();
                        Some((id, name))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// `active_cons` / `max_connections` out of a `player_api.php` `user_info`
/// blob. This is the authoritative answer to "is the line free?" - the roster
/// only says *who*, the panel says *whether*.
pub fn account_counts(user_info: &Value) -> (i64, i64) {
    (int_at(user_info, "active_cons"), int_at(user_info, "max_connections"))
}

fn short(s: &str) -> String {
    let cut = s.split('\n').next().unwrap_or(s).trim();
    if cut.len() > 120 {
        format!("{}…", &cut[..119])
    } else {
        cut.to_string()
    }
}

fn parse_roster(v: Value) -> Result<Roster, String> {
    let account = v.get("account");
    let watchers = v
        .get("watchers")
        .and_then(|w| w.as_array())
        .map(|arr| {
            arr.iter()
                .map(|w| Watcher {
                    profile: str_at(w, "profile"),
                    name: str_at(w, "name"),
                    device: str_at(w, "device"),
                    state: str_at(w, "state"),
                    stream_name: str_at(w, "streamName"),
                    age_seconds: int_at(w, "ageSeconds"),
                    watching_seconds: int_at(w, "watchingSeconds"),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Roster {
        watchers,
        active_cons: account.map(|a| int_at(a, "activeCons")).unwrap_or(0),
        max_connections: account.map(|a| int_at(a, "maxConnections")).unwrap_or(0),
        unaccounted: account.map(|a| int_at(a, "unaccounted")).unwrap_or(0),
        fetched_at: now_ms(),
        profiles: Vec::new(),
        error: None,
    })
}

fn str_at(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn int_at(v: &Value, key: &str) -> i64 {
    match v.get(key) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        // The panel (and therefore the worker, for the account block) may hand
        // numbers back as strings.
        Some(Value::String(s)) => s.parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

/* ------------------------------------------------------------------- thread */

pub fn spawn(shared: Arc<Mutex<Shared>>, roster: Arc<Mutex<Roster>>) {
    std::thread::spawn(move || run(shared, roster));
}

fn publish(slot: &Arc<Mutex<Roster>>, value: Roster) {
    if let Ok(mut g) = slot.lock() {
        // Keep the profile list we may already have; it is fetched separately
        // and does not change between heartbeats.
        let profiles = std::mem::take(&mut g.profiles);
        *g = value;
        g.profiles = profiles;
    }
}

fn publish_error(slot: &Arc<Mutex<Roster>>, err: String) {
    if let Ok(mut g) = slot.lock() {
        g.error = Some(err);
    }
}

fn run(shared: Arc<Mutex<Shared>>, roster: Arc<Mutex<Roster>>) {
    let mut last_heartbeat = 0i64;
    let mut last_poll = 0i64;
    let mut last_account = 0i64;
    let mut last_signature = String::new();
    let mut sent_active = false;
    let mut profiles_loaded = false;

    // A short initial wait keeps app startup snappy: the worker thread is
    // competing for nothing, but the panel login thread should win.
    std::thread::sleep(Duration::from_millis(1500));

    loop {
        let snap = match shared.lock() {
            Ok(g) => g.clone(),
            Err(_) => break,
        };
        if snap.shutdown {
            break;
        }

        if !snap.enabled || snap.profile.is_empty() || snap.device_id.is_empty() || snap.url.is_empty()
        {
            std::thread::sleep(Duration::from_millis(TICK_MS));
            continue;
        }

        let client = Presence::new(&snap.url, &snap.token);

        // A profile the user abandoned in settings - forget it immediately
        // rather than leaving a ghost entry for the rest of the TTL.
        if !snap.stale_profile.is_empty() {
            let _ = client.idle(&snap.stale_profile, &snap.device_id);
            if let Ok(mut g) = shared.lock() {
                g.stale_profile.clear();
            }
        }

        if snap.force_poll {
            last_poll = 0;
            last_heartbeat = 0;
            if let Ok(mut g) = shared.lock() {
                g.force_poll = false;
            }
        }
        if snap.force_account {
            last_account = 0;
            if let Ok(mut g) = shared.lock() {
                g.force_account = false;
            }
        }

        if !profiles_loaded {
            if let Ok(list) = client.profiles() {
                if !list.is_empty() {
                    if let Ok(mut g) = roster.lock() {
                        g.profiles = list;
                    }
                    profiles_loaded = true;
                }
            }
        }

        // Refresh the connection counters from the panel. This is the same
        // player_api.php call the app already makes at login, so it costs one
        // extra request per 90s and is what makes "someone is watching who is
        // not one of us" detectable at all.
        let now = now_ms();
        if now - last_account >= ACCOUNT_REFRESH_MS
            && !snap.server.is_empty()
            && !snap.user.is_empty()
            && !snap.pass.is_empty()
        {
            last_account = now;
            if let Ok(panel) = crate::Xtream::new(&snap.server, &snap.user, &snap.pass) {
                if let Ok(info) = panel.authenticate() {
                    let ac = int_at(&info, "active_cons");
                    let mc = int_at(&info, "max_connections");
                    if let Ok(mut g) = shared.lock() {
                        if ac > 0 {
                            g.active_cons = ac;
                        }
                        if mc > 0 {
                            g.max_connections = mc;
                        }
                    }
                }
            }
        }

        if snap.active {
            let sig = snap.signature();
            let heartbeat_due = now - last_heartbeat >= HEARTBEAT_MS || sig != last_signature;
            if heartbeat_due {
                match client.heartbeat(&snap) {
                    Ok(r) => {
                        publish(&roster, r);
                        last_heartbeat = now;
                        last_poll = now;
                        last_signature = sig;
                        sent_active = true;
                    }
                    Err(e) => publish_error(&roster, e),
                }
            } else if now - last_poll >= POLL_ACTIVE_MS {
                // A heartbeat is a KV *write*, so the 60s cadence above is
                // deliberately lazy. A plain roster read is cheap, so the
                // "who's on" view still refreshes every 20s while watching.
                last_poll = now;
                match client.roster() {
                    Ok(r) => publish(&roster, r),
                    Err(e) => publish_error(&roster, e),
                }
            }
        } else {
            if sent_active {
                // Best-effort courtesy clear; the KV TTL covers it if this
                // request never lands.
                let _ = client.idle(&snap.profile, &snap.device_id);
                sent_active = false;
                last_signature.clear();
            }
            if now - last_poll >= POLL_IDLE_MS {
                last_poll = now;
                match client.roster() {
                    Ok(r) => publish(&roster, r),
                    Err(e) => publish_error(&roster, e),
                }
            }
        }

        std::thread::sleep(Duration::from_millis(TICK_MS));
    }
}

/// Called on app exit. Blocking on purpose, with a short timeout: the process
/// is going away, and a stale roster entry is the thing we are trying to avoid.
pub fn clear_on_exit(shared: &Arc<Mutex<Shared>>) {
    let snap = match shared.lock() {
        Ok(mut g) => {
            g.shutdown = true;
            g.clone()
        }
        Err(_) => return,
    };
    if !snap.enabled || !snap.active || snap.url.is_empty() {
        return;
    }
    let _ = Presence::new(&snap.url, &snap.token).idle(&snap.profile, &snap.device_id);
}

/// "12m", "1h 04m" - for the hover text and the dashboard mirror.
pub fn human_duration(seconds: i64) -> String {
    let s = seconds.max(0);
    if s < 60 {
        return format!("{}s", s);
    }
    let m = s / 60;
    if m < 60 {
        return format!("{}m", m);
    }
    format!("{}h {:02}m", m / 60, m % 60)
}
