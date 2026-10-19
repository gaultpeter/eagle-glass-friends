//! Hand-off from the TV guide page to the app.
//!
//! The guide is a web page; the credentials are not. Clicking a listing sends
//! `eagleglass://play?id=91202&name=Sky%20Sports`, the app resolves that id
//! against the panel it is already logged into, and plays it.
//!
//! Three pieces, all Windows-specific and none of them optional:
//!
//! 1. **Scheme registration.** Windows has to know what `eagleglass://` means, so
//!    a `URL Protocol` key is written under `HKCU\Software\Classes`. No admin
//!    rights, and deleting the key undoes it.
//! 2. **Single instance.** Clicking a link while the app is already running makes
//!    Windows start a *second* copy. On a line that allows one connection those
//!    two copies would kick each other off, so the second one hands its request
//!    to the first and exits.
//! 3. **The hand-over.** A tiny JSON file in the temp directory. A mutex is the
//!    signal, the file is the payload, and the running instance picks it up on
//!    its next tick. No sockets, no window messages to subclass.

use serde_json::{json, Value};
use std::path::PathBuf;

/// A channel to open, as asked for by the guide page.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayRequest {
    pub id: String,
    pub name: String,
    /// The panel's category id, when the link knows it. Optional: an older link,
    /// or one typed by hand, simply will not move the sidebar.
    pub cat: Option<String>,
}

/// What the running instance has been asked to do.
#[derive(Clone, Debug)]
pub enum Request {
    Play(PlayRequest),
    /// Bring the window forward - what a plain double-click of the exe means
    /// once a second instance is refused.
    Focus,
}

/// `eagleglass://play?id=91202&name=Sky%20Sports%20Main%20Event`
///
/// The id is required to be bare digits. It ends up inside a URL path on the
/// panel (`live/<user>/<pass>/<id>.ts`), so anything else would be a way to
/// steer that request somewhere else.
pub fn parse_uri(arg: &str) -> Option<PlayRequest> {
    // URL schemes are case-insensitive, and Windows hands back whatever the
    // caller wrote, so compare the prefix without touching the rest - the query
    // string carries a channel name that must keep its own casing.
    const PREFIX: &str = "eagleglass://";
    if arg.len() < PREFIX.len()
        || !arg.is_char_boundary(PREFIX.len())
        || !arg[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
    {
        return None;
    }
    let rest = &arg[PREFIX.len()..];
    let (route, query) = match rest.split_once('?') {
        Some((r, q)) => (r, q),
        None => (rest, ""),
    };
    if !route.trim_end_matches('/').eq_ignore_ascii_case("play") {
        return None;
    }

    let mut id = String::new();
    let mut name = String::new();
    let mut cat = String::new();
    for pair in query.split('&') {
        let (k, v) = match pair.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        let value = percent_decode(v);
        match percent_decode(k).to_ascii_lowercase().as_str() {
            "id" => id = value,
            "name" => name = value,
            // A category id is numeric on every Xtream panel; anything else is
            // not passed on.
            "cat" if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) => cat = value,
            _ => {}
        }
    }

    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let name = if name.trim().is_empty() {
        format!("Stream {}", id)
    } else {
        name.trim().to_string()
    };
    Some(PlayRequest {
        id,
        name,
        cat: if cat.is_empty() { None } else { Some(cat) },
    })
}

/// Percent-decoding, plus `+` as space, which is what a form-style query uses.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/* ------------------------------------------------------------ the hand-over */

fn request_path() -> PathBuf {
    std::env::temp_dir().join("eglass-request.json")
}

/// Hand a request to whichever instance already owns the mutex.
pub fn send(req: &Request) -> std::io::Result<()> {
    let body = match req {
        Request::Focus => json!({ "kind": "focus" }),
        Request::Play(p) => json!({ "kind": "play", "id": p.id, "name": p.name, "cat": p.cat }),
    };
    std::fs::write(request_path(), body.to_string())
}

/// Pick up a request, if one is waiting and readable.
///
/// A half-written file is left in place rather than deleted: the writer is
/// another process and a retry costs nothing.
pub fn take() -> Option<Request> {
    let path = request_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let _ = std::fs::remove_file(&path);
    match value.get("kind").and_then(|k| k.as_str()) {
        Some("focus") => Some(Request::Focus),
        Some("play") => {
            let id = value.get("id").and_then(|v| v.as_str())?.to_string();
            if !id.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let name = value
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // A category that is not a plain number is not one of ours.
            let cat = value
                .get("cat")
                .and_then(|v| v.as_str())
                .filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_digit()))
                .map(|c| c.to_string());
            Some(Request::Play(PlayRequest { id, name, cat }))
        }
        _ => None,
    }
}

/* ------------------------------------------------------------- windows bits */

/// Claim the single-instance mutex.
///
/// `true` means this process is the one that owns it and should run the GUI.
/// The handle is deliberately leaked: dropping it would release the mutex while
/// the app is still alive, and the next click would start a second window.
#[cfg(windows)]
pub fn claim_single_instance() -> bool {
    use std::sync::OnceLock;
    static HANDLE: OnceLock<isize> = OnceLock::new();
    let name = wide("Local\\EagleGlassSingleInstance");
    let handle = unsafe { win::CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
    let already = unsafe { win::GetLastError() } == win::ERROR_ALREADY_EXISTS;
    let _ = HANDLE.set(handle as isize);
    !already
}

#[cfg(not(windows))]
pub fn claim_single_instance() -> bool {
    true
}

/// Teach Windows what `eagleglass://` means, so a link in a browser can reach
/// the app. Idempotent, current-user only, and reversible by deleting the key.
#[cfg(windows)]
pub fn register_uri_scheme() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let command = format!("\"{}\" \"%1\"", exe.display());

    let root = win::open_key(win::HKEY_CURRENT_USER, r"Software\Classes\eagleglass")?;
    win::set_string(root, "", "URL:Eagle Glass")?;
    win::set_string(root, "URL Protocol", "")?;
    win::close(root);

    let cmd = win::open_key(
        win::HKEY_CURRENT_USER,
        r"Software\Classes\eagleglass\shell\open\command",
    )?;
    win::set_string(cmd, "", &command)?;
    win::close(cmd);
    Ok(())
}

#[cfg(not(windows))]
pub fn register_uri_scheme() -> Result<(), String> {
    Err("not supported on this platform".into())
}

#[cfg(windows)]
pub fn scheme_is_registered() -> bool {
    win::open_key(win::HKEY_CURRENT_USER, r"Software\Classes\eagleglass\shell\open\command").is_ok()
}

#[cfg(not(windows))]
pub fn scheme_is_registered() -> bool {
    false
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    pub const HKEY_CURRENT_USER: usize = 0x8000_0001;
    pub const KEY_WRITE: u32 = 0x2_0006;
    pub const REG_SZ: u32 = 1;
    pub const ERROR_ALREADY_EXISTS: u32 = 183;

    pub type Handle = *mut c_void;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegCreateKeyExW(
            key: Handle,
            subkey: *const u16,
            reserved: u32,
            class: *const u16,
            options: u32,
            access: u32,
            security: *mut c_void,
            out: *mut Handle,
            disposition: *mut u32,
        ) -> i32;
        fn RegSetValueExW(
            key: Handle,
            name: *const u16,
            reserved: u32,
            ty: u32,
            data: *const u8,
            len: u32,
        ) -> i32;
        fn RegCloseKey(key: Handle) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn CreateMutexW(security: *mut c_void, initial_owner: i32, name: *const u16) -> Handle;
        pub fn GetLastError() -> u32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn open_key(root: usize, path: &str) -> Result<Handle, String> {
        let mut out: Handle = std::ptr::null_mut();
        let code = unsafe {
            RegCreateKeyExW(
                root as Handle,
                wide(path).as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_WRITE,
                std::ptr::null_mut(),
                &mut out,
                std::ptr::null_mut(),
            )
        };
        if code != 0 {
            return Err(format!("RegCreateKeyExW({path}) failed with {code}"));
        }
        Ok(out)
    }

    pub fn set_string(key: Handle, name: &str, value: &str) -> Result<(), String> {
        let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let code = unsafe {
            RegSetValueExW(
                key,
                wide(name).as_ptr(),
                0,
                REG_SZ,
                data.as_ptr() as *const u8,
                (data.len() * 2) as u32,
            )
        };
        if code != 0 {
            return Err(format!("RegSetValueExW({name}) failed with {code}"));
        }
        Ok(())
    }

    pub fn close(key: Handle) {
        unsafe {
            RegCloseKey(key);
        }
    }
}

/// Serialises the tests that touch the hand-over file.
///
/// That file has a fixed path, and a test that drives a real `App` has to
/// redirect the whole environment to keep away from the user's settings - which
/// also moves this file. Anything that both writes and reads it in one test has
/// to hold this, or a parallel test can read the file out from under it.
#[cfg(test)]
pub fn test_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guide_link_parses() {
        let r = parse_uri("eagleglass://play?id=91202&name=Sky%20Sports%20Main%20Event").unwrap();
        assert_eq!(r.id, "91202");
        assert_eq!(r.name, "Sky Sports Main Event");
        assert_eq!(r.cat, None);
    }

    #[test]
    fn a_link_can_carry_the_category_so_the_sidebar_can_follow() {
        let r = parse_uri("eagleglass://play?id=91202&name=Sky&cat=17").unwrap();
        assert_eq!(r.cat.as_deref(), Some("17"));
    }

    // The exact shape the guide page builds, including an encoded name.
    #[test]
    fn the_link_the_page_actually_sends() {
        let r = parse_uri("eagleglass://play?id=81564&name=AUS%3A%20FOX%20SPORTS%20501&cat=83").unwrap();
        assert_eq!(r.id, "81564");
        assert_eq!(r.name, "AUS: FOX SPORTS 501");
        assert_eq!(r.cat.as_deref(), Some("83"));
    }

    #[test]
    fn a_junk_category_is_ignored_rather_than_passed_on() {
        assert_eq!(parse_uri("eagleglass://play?id=5&cat=../../etc").unwrap().cat, None);
        assert_eq!(parse_uri("eagleglass://play?id=5&cat=").unwrap().cat, None);
        assert_eq!(parse_uri("eagleglass://play?id=5&cat=abc").unwrap().cat, None);
    }

    #[test]
    fn the_name_is_optional() {
        let r = parse_uri("eagleglass://play?id=101").unwrap();
        assert_eq!(r.id, "101");
        assert_eq!(r.name, "Stream 101");
    }

    #[test]
    fn accents_survive_the_query_string() {
        let r = parse_uri("eagleglass://play?id=7&name=Caf%C3%A9%20Society").unwrap();
        assert_eq!(r.name, "Café Society");
    }

    #[test]
    fn plus_means_space() {
        assert_eq!(percent_decode("Sky+Sports"), "Sky Sports");
    }

    // The id goes into a panel URL path, so a non-numeric one is refused rather
    // than sanitised.
    #[test]
    fn a_non_numeric_id_is_refused() {
        assert!(parse_uri("eagleglass://play?id=../../etc/passwd").is_none());
        assert!(parse_uri("eagleglass://play?id=101.ts").is_none());
        assert!(parse_uri("eagleglass://play?name=Nope").is_none());
        assert!(parse_uri("eagleglass://play?id=").is_none());
    }

    #[test]
    fn other_routes_and_schemes_are_ignored() {
        assert!(parse_uri("eagleglass://settings?id=1").is_none());
        assert!(parse_uri("https://example.com/play?id=1").is_none());
        assert!(parse_uri("--autoplay").is_none());
    }

    #[test]
    fn the_uri_scheme_is_case_insensitive() {
        assert!(parse_uri("EagleGlass://PLAY?id=5").is_some());
        assert!(parse_uri("eagleglass://Play?id=5").is_some());
        // ...but the channel name must not be folded to lower case with it
        let r = parse_uri("EAGLEGLASS://play?id=5&name=Sky%20Sports").unwrap();
        assert_eq!(r.name, "Sky Sports");
    }

    #[test]
    fn a_multi_byte_arg_does_not_panic() {
        // the prefix check must not slice into the middle of a character
        assert!(parse_uri("eagleglas☃").is_none());
        assert!(parse_uri("☃☃☃☃☃☃☃☃☃☃☃☃☃☃").is_none());
    }

    // The hand-over file has a fixed path, so everything that touches it lives in
    // one test - Rust runs tests in parallel and they would otherwise race. The
    // lock is held because another test redirects the environment underneath it.
    #[test]
    fn the_handover_file_behaves() {
        let _serial = test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let path = request_path();
        let _ = std::fs::remove_file(&path);

        let want = PlayRequest { id: "91202".into(), name: "Sky Sports".into(), cat: Some("17".into()) };
        send(&Request::Play(want.clone())).unwrap();
        match take() {
            Some(Request::Play(got)) => assert_eq!(got, want),
            other => panic!("expected a play request, got {other:?}"),
        }
        assert!(take().is_none(), "a request must be consumed once");

        send(&Request::Focus).unwrap();
        assert!(matches!(take(), Some(Request::Focus)));

        // a half-written file is left for the next tick rather than discarded
        std::fs::write(&path, "{ this is not json").unwrap();
        assert!(take().is_none());
        assert!(path.exists(), "an unreadable request should be retried, not dropped");

        // and a well-formed file with a hostile id is refused outright
        std::fs::write(&path, r#"{"kind":"play","id":"..%2Fetc","name":"x"}"#).unwrap();
        assert!(take().is_none());
        let _ = std::fs::remove_file(&path);
    }
}
