use serde_json::Value;
use std::io::Read;
use std::time::Duration;

pub const DEFAULT_SERVER: &str = "";
const USER_AGENT: &str = "EagleGlassRS/0.1";

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn field(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string().trim_matches('"').to_string(),
    }
}

#[derive(Clone)]
pub struct Category {
    pub id: String,
    pub name: String,
}

#[derive(Clone)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub ext: String,
    pub cat: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Live,
    Vod,
    Series,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Live => "Live TV",
            Kind::Vod => "Movies",
            Kind::Series => "Series",
        }
    }
}

pub struct Xtream {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    pub user: String,
    pub pass: String,
    pub timeout: u64,
}

impl Xtream {
    pub fn new(server: &str, user: &str, pass: &str) -> Result<Self, String> {
        let mut s = server.trim().to_string();
        if s.is_empty() {
            return Err("Server address is required".into());
        }
        if !s.starts_with("http://") && !s.starts_with("https://") {
            s = format!("http://{}", s);
        }
        let (scheme, rest) = match s.strip_prefix("https://") {
            Some(r) => ("https", r),
            None => ("http", s.strip_prefix("http://").unwrap_or(&s)),
        };
        let authority = rest.split('/').next().unwrap_or(rest).to_string();
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
                (h.to_string(), p.parse::<u16>().ok())
            }
            _ => (authority, None),
        };
        if host.is_empty() {
            return Err("Could not parse a host from the server address".into());
        }
        Ok(Xtream {
            scheme: scheme.to_string(),
            host,
            port,
            user: user.trim().to_string(),
            pass: pass.to_string(),
            timeout: 25,
        })
    }

    pub fn base(&self) -> String {
        match self.port {
            Some(p) => format!("{}://{}:{}", self.scheme, self.host, p),
            None => format!("{}://{}", self.scheme, self.host),
        }
    }

    fn raw_get(&self, url: &str) -> Result<String, String> {
        let resp = ureq::builder()
            .timeout(Duration::from_secs(self.timeout))
            .build()
            .get(url)
            .set("User-Agent", USER_AGENT)
            .call()
            .map_err(|e| format!("Request failed for {}: {}", url, e))?;
        let mut text = String::new();
        resp.into_reader()
            .read_to_string(&mut text)
            .map_err(|e| e.to_string())?;
        Ok(text)
    }

    fn api(&self, action: Option<&str>, extra: &[(&str, &str)]) -> Result<Value, String> {
        let mut url = format!(
            "{}/player_api.php?username={}&password={}",
            self.base(),
            urlencode(&self.user),
            urlencode(&self.pass)
        );
        if let Some(a) = action {
            url.push_str(&format!("&action={}", urlencode(a)));
        }
        for (k, v) in extra {
            url.push_str(&format!("&{}={}", urlencode(k), urlencode(v)));
        }
        let text = self.raw_get(&url)?;
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed == "null" {
            return Ok(Value::Null);
        }
        serde_json::from_str(trimmed).map_err(|_| "Server did not return JSON (not an Xtream panel?)".to_string())
    }

    pub fn authenticate(&self) -> Result<Value, String> {
        let v = self.api(None, &[])?;
        let ui = v.get("user_info").cloned().unwrap_or(Value::Null);
        if ui.is_null() {
            return Err("Unexpected login response".into());
        }
        let auth = field(&ui, "auth");
        let status = field(&ui, "status").to_lowercase();
        if auth == "0" || status == "disabled" || status == "expired" || status == "banned" {
            return Err(format!("Login rejected (auth={}, status={})", field(&ui, "auth"), field(&ui, "status")));
        }
        Ok(ui)
    }

    fn categories(&self, action: &str) -> Result<Vec<Category>, String> {
        let v = self.api(Some(action), &[])?;
        let mut out = Vec::new();
        if let Some(arr) = v.as_array() {
            for c in arr {
                out.push(Category {
                    id: field(c, "category_id"),
                    name: field(c, "category_name"),
                });
            }
        }
        Ok(out)
    }

    fn streams(&self, action: &str, cat: Option<&str>, id_key: &str, ext: bool) -> Result<Vec<Item>, String> {
        let extra: Vec<(&str, &str)> = match cat {
            Some(c) if !c.is_empty() => vec![("category_id", c)],
            _ => vec![],
        };
        let v = self.api(Some(action), &extra)?;
        let mut out = Vec::new();
        if let Some(arr) = v.as_array() {
            for s in arr {
                out.push(Item {
                    id: field(s, id_key),
                    name: field(s, "name"),
                    ext: if ext { field(s, "container_extension") } else { String::new() },
                    cat: field(s, "category_id"),
                });
            }
        }
        Ok(out)
    }

    pub fn live_categories(&self) -> Result<Vec<Category>, String> {
        self.categories("get_live_categories")
    }
    pub fn vod_categories(&self) -> Result<Vec<Category>, String> {
        self.categories("get_vod_categories")
    }
    pub fn series_categories(&self) -> Result<Vec<Category>, String> {
        self.categories("get_series_categories")
    }

    pub fn category_items(&self, kind: Kind, cat: Option<&str>) -> Result<Vec<Item>, String> {
        match kind {
            Kind::Live => self.streams("get_live_streams", cat, "stream_id", false),
            Kind::Vod => self.streams("get_vod_streams", cat, "stream_id", true),
            Kind::Series => self.streams("get_series", cat, "series_id", false),
        }
    }

    pub fn first_episode(&self, series_id: &str) -> Result<Option<(String, String)>, String> {
        let v = self.api(Some("get_series_info"), &[("series_id", series_id)])?;
        if let Some(eps) = v.get("episodes").and_then(|e| e.as_object()) {
            let mut seasons: Vec<&String> = eps.keys().collect();
            seasons.sort_by_key(|k| k.parse::<u32>().unwrap_or(0));
            for s in seasons {
                if let Some(list) = eps.get(s).and_then(|x| x.as_array()) {
                    if let Some(ep) = list.first() {
                        return Ok(Some((field(ep, "id"), field(ep, "container_extension"))));
                    }
                }
            }
        }
        Ok(None)
    }

    fn stream_url(&self, seg: &str, id: &str, ext: &str) -> String {
        let ext = if ext.is_empty() { String::new() } else { format!(".{}", ext) };
        format!("{}/{}/{}/{}/{}{}", self.base(), seg, urlencode(&self.user), urlencode(&self.pass), id, ext)
    }

    pub fn live_url(&self, id: &str, format: &str) -> String {
        self.stream_url("live", id, format)
    }
    pub fn vod_url(&self, id: &str, ext: &str) -> String {
        let e = if ext.is_empty() { "mp4" } else { ext };
        self.stream_url("movie", id, e)
    }
    pub fn series_url(&self, episode_id: &str, ext: &str) -> String {
        let e = if ext.is_empty() { "mp4" } else { ext };
        self.stream_url("series", episode_id, e)
    }

    pub fn play_url(&self, kind: Kind, item: &Item, live_format: &str) -> Result<String, String> {
        match kind {
            Kind::Live => Ok(self.live_url(&item.id, live_format)),
            Kind::Vod => Ok(self.vod_url(&item.id, &item.ext)),
            Kind::Series => match self.first_episode(&item.id)? {
                Some((id, ext)) => Ok(self.series_url(&id, &ext)),
                None => Err("No episodes found for this series".into()),
            },
        }
    }

    pub fn m3u_url(&self, kind: &str, output: &str) -> String {
        format!(
            "{}/get.php?username={}&password={}&type={}&output={}",
            self.base(),
            urlencode(&self.user),
            urlencode(&self.pass),
            kind,
            output
        )
    }

    pub fn xmltv_url(&self) -> String {
        format!("{}/xmltv.php?username={}&password={}", self.base(), urlencode(&self.user), urlencode(&self.pass))
    }

    pub fn m3u_playlist(&self) -> Result<String, String> {
        self.raw_get(&self.m3u_url("m3u_plus", "ts"))
    }
}
