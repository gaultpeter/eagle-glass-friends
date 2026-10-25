mod launch;
mod presence;
mod video;
mod xtream;

use eframe::egui;
use raw_window_handle::HasWindowHandle;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

use presence::{Roster, Shared};
use xtream::{Item, Kind, Xtream, DEFAULT_SERVER};

const T_BG: egui::Color32 = egui::Color32::from_rgb(0x10, 0x12, 0x17);
const T_SIDEBAR: egui::Color32 = egui::Color32::from_rgb(0x17, 0x1a, 0x20);
const T_TOP: egui::Color32 = egui::Color32::from_rgb(0x19, 0x1c, 0x23);
const T_SURFACE: egui::Color32 = egui::Color32::from_rgb(0x20, 0x24, 0x2d);
const T_SURFACE2: egui::Color32 = egui::Color32::from_rgb(0x29, 0x2e, 0x38);
const T_BORDER: egui::Color32 = egui::Color32::from_rgb(0x30, 0x35, 0x41);
const T_TEXT: egui::Color32 = egui::Color32::from_rgb(0xf1, 0xf3, 0xf8);
const T_MUTED: egui::Color32 = egui::Color32::from_rgb(0x9a, 0xa4, 0xb5);
const T_ACCENT: egui::Color32 = egui::Color32::from_rgb(0x89, 0x98, 0xff);
const T_ACCENT_SOFT: egui::Color32 = egui::Color32::from_rgb(0x30, 0x37, 0x50);
const T_DANGER: egui::Color32 = egui::Color32::from_rgb(0xff, 0x73, 0x7a);
const T_SUCCESS: egui::Color32 = egui::Color32::from_rgb(0x56, 0xd6, 0x9a);

fn pill(ui: &mut egui::Ui, color: egui::Color32, text: &str) {
    let _ = pill_resp(ui, color, text);
}

/// Same pill, but hands back the response so callers can attach hover text.
fn pill_resp(ui: &mut egui::Ui, color: egui::Color32, text: &str) -> egui::Response {
    egui::Frame::none()
        .fill(T_SURFACE)
        .rounding(egui::Rounding::same(999.0))
        .inner_margin(egui::Margin::symmetric(10.0, 4.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 4.5, color);
                ui.add(egui::Label::new(egui::RichText::new(text).size(12.0).color(T_TEXT)));
            });
        })
        .response
}

fn config_path() -> Option<std::path::PathBuf> {
    let base = std::env::var("APPDATA")
        .ok()
        .or_else(|| std::env::var("HOME").ok().map(|h| format!("{}/.config", h)))?;
    Some(std::path::Path::new(&base).join("EagleGlass").join("config.txt"))
}

/// Everything kept on disk. This used to be a 4-tuple written by a function
/// that emitted exactly four lines, which would silently erase any extra
/// setting the moment the settings dialog was used - so presence config has to
/// live in the same struct that gets written back.
#[derive(Clone, Default)]
struct Config {
    server: String,
    user: String,
    pass: String,
    format: String,
    profile: String,
    device_id: String,
    presence_url: String,
    presence_token: String,
    /// Last volume the user settled on, 0-100. 100 is the default only for a
    /// config that predates this field.
    volume: i32,
    /// Picture settings remembered per stream, so a channel you tuned comes back
    /// the way you left it.
    pictures: Vec<Picture>,
}

/// Brightness/contrast/gamma for one stream.
#[derive(Clone)]
struct Picture {
    /// Stream id, which is what the panel uses and what a client asks for.
    id: String,
    /// Kept only so the config file is readable when you open it.
    name: String,
    bright: f32,
    contrast: f32,
    gamma: f32,
}

/// Slider ranges, shared by the UI and the config loader so a hand-edited file
/// cannot put a slider out of range.
const PIC_BRIGHT: std::ops::RangeInclusive<f32> = 0.5..=2.0;
const PIC_GAMMA: std::ops::RangeInclusive<f32> = 0.5..=3.0;
/// Enough for any realistic amount of tuning; the oldest entries drop first.
const PIC_LIMIT: usize = 200;

fn clamp_picture(bright: f32, contrast: f32, gamma: f32) -> (f32, f32, f32) {
    (
        bright.clamp(*PIC_BRIGHT.start(), *PIC_BRIGHT.end()),
        contrast.clamp(*PIC_BRIGHT.start(), *PIC_BRIGHT.end()),
        gamma.clamp(*PIC_GAMMA.start(), *PIC_GAMMA.end()),
    )
}

/// What a stream should look like: its saved settings, or untouched defaults.
fn picture_for(pictures: &[Picture], id: &str) -> (f32, f32, f32) {
    match pictures.iter().find(|p| p.id == id) {
        Some(p) => (p.bright, p.contrast, p.gamma),
        None => (1.0, 1.0, 1.0),
    }
}

/// Insert or replace the entry for a stream, newest last so that a full list
/// drops the least recently tuned rather than the oldest stream id.
fn upsert_picture(
    pictures: &mut Vec<Picture>,
    id: &str,
    name: &str,
    bright: f32,
    contrast: f32,
    gamma: f32,
) {
    // Three decimals: enough to be exact on screen, short enough that the config
    // file stays readable when someone opens it.
    let round = |v: f32| (v * 1000.0).round() / 1000.0;
    let (bright, contrast, gamma) = clamp_picture(round(bright), round(contrast), round(gamma));
    pictures.retain(|p| p.id != id);
    pictures.push(Picture {
        id: id.to_string(),
        name: name.to_string(),
        bright,
        contrast,
        gamma,
    });
    if pictures.len() > PIC_LIMIT {
        let excess = pictures.len() - PIC_LIMIT;
        pictures.drain(0..excess);
    }
}

fn load_config() -> Config {
    // Note the explicit volume default: a plain derive would make it 0 and a
    // fresh instance would start silent, which is its own kind of surprise.
    let mut cfg = Config {
        volume: 100,
        ..Default::default()
    };
    let p = match config_path() {
        Some(p) => p,
        None => return cfg,
    };
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(_) => return cfg,
    };
    // A UTF-8 BOM would otherwise become part of the first key, silently
    // dropping that setting. Notepad writes one, so this is not hypothetical.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    for line in text.lines() {
        let mut it = line.splitn(2, '=');
        let (key, value) = match (it.next(), it.next()) {
            (Some(k), Some(v)) => (k.trim(), v.to_string()),
            _ => continue,
        };
        match key {
            "server" => cfg.server = value,
            "user" => cfg.user = value,
            "pass" => cfg.pass = value,
            "format" => cfg.format = value,
            "profile" => cfg.profile = value,
            "device_id" => cfg.device_id = value,
            "presence_url" => cfg.presence_url = value,
            "presence_token" => cfg.presence_token = value,
            "volume" => {
                if let Ok(v) = value.trim().parse::<i32>() {
                    cfg.volume = v.clamp(0, 100);
                }
            }
            "pic" => {
                // pic=<stream id>|<brightness>|<contrast>|<gamma>|<name>
                let parts: Vec<&str> = value.splitn(5, '|').collect();
                if parts.len() >= 4 {
                    if let (Ok(b), Ok(c), Ok(g)) = (
                        parts[1].trim().parse::<f32>(),
                        parts[2].trim().parse::<f32>(),
                        parts[3].trim().parse::<f32>(),
                    ) {
                        let (bright, contrast, gamma) = clamp_picture(b, c, g);
                        cfg.pictures.push(Picture {
                            id: parts[0].trim().to_string(),
                            name: parts.get(4).map(|s| s.trim().to_string()).unwrap_or_default(),
                            bright,
                            contrast,
                            gamma,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    cfg
}

fn save_config(cfg: &Config) {
    if let Some(p) = config_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut data = format!(
            "server={}\nuser={}\npass={}\nformat={}\nprofile={}\ndevice_id={}\npresence_url={}\npresence_token={}\nvolume={}\n",
            cfg.server,
            cfg.user,
            cfg.pass,
            cfg.format,
            cfg.profile,
            cfg.device_id,
            cfg.presence_url,
            cfg.presence_token,
            cfg.volume
        );
        // One line per tuned stream. Kept last so the familiar settings stay at
        // the top of the file.
        for p in &cfg.pictures {
            data.push_str(&format!(
                "pic={}|{:.3}|{:.3}|{:.3}|{}\n",
                p.id, p.bright, p.contrast, p.gamma, p.name
            ));
        }
        let _ = std::fs::write(&p, data);
    }
}

fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = T_BG;
    v.window_fill = T_SIDEBAR;
    v.extreme_bg_color = T_SIDEBAR;
    v.faint_bg_color = T_SURFACE;
    v.override_text_color = Some(T_TEXT);
    v.window_stroke = egui::Stroke::new(1.0_f32, T_BORDER);
    v.hyperlink_color = T_ACCENT;
    v.selection.bg_fill = T_ACCENT_SOFT;
    v.selection.stroke = egui::Stroke::new(1.0_f32, T_ACCENT);

    let r = egui::Rounding::same(6.0);
    v.widgets.noninteractive.bg_fill = T_SIDEBAR;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, T_BORDER);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, T_MUTED);
    v.widgets.noninteractive.rounding = r;
    v.widgets.inactive.bg_fill = T_SURFACE2;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, T_BORDER);
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, T_TEXT);
    v.widgets.inactive.rounding = r;
    v.widgets.hovered.bg_fill = T_SURFACE;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, T_ACCENT);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, T_TEXT);
    v.widgets.hovered.rounding = r;
    v.widgets.active.bg_fill = T_ACCENT_SOFT;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, T_ACCENT);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, T_TEXT);
    v.widgets.active.rounding = r;
    v.widgets.open.bg_fill = T_SURFACE;
    v.widgets.open.bg_stroke = egui::Stroke::new(1.0_f32, T_BORDER);
    v.widgets.open.fg_stroke = egui::Stroke::new(1.0_f32, T_TEXT);
    v.widgets.open.rounding = r;
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
    });
}

fn truncate_text(ui: &egui::Ui, s: &str, font: &egui::FontId, max: f32) -> String {
    let w = ui.fonts(|f| f.layout_no_wrap(s.to_string(), font.clone(), T_TEXT)).size().x;
    if w <= max {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars() {
        let mut probe = out.clone();
        probe.push(ch);
        probe.push('…');
        if ui.fonts(|f| f.layout_no_wrap(probe, font.clone(), T_TEXT)).size().x > max {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

fn side_header(ui: &mut egui::Ui, open: bool, label: &str, count: usize) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, T_SURFACE);
    }
    let chev = if open { "v" } else { ">" };
    ui.painter().text(
        egui::pos2(rect.min.x + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        chev,
        egui::FontId::proportional(12.0),
        T_MUTED,
    );
    let font = egui::FontId::proportional(11.5);
    let text = truncate_text(ui, label, &font, rect.width() - 58.0);
    ui.painter().text(
        egui::pos2(rect.min.x + 22.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        font,
        T_TEXT,
    );
    ui.painter().text(
        egui::pos2(rect.max.x - 9.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        egui::FontId::proportional(11.0),
        T_MUTED,
    );
    resp.clicked()
}

fn side_row(ui: &mut egui::Ui, selected: bool, label: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 5.0, T_ACCENT_SOFT);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(2.5, rect.height())),
            1.0,
            T_ACCENT,
        );
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, T_SURFACE);
    }
    let color = if selected || resp.hovered() { T_TEXT } else { T_MUTED };
    let font = egui::FontId::proportional(12.5);
    let text = truncate_text(ui, label, &font, rect.width() - 28.0);
    ui.painter().text(
        egui::pos2(rect.min.x + 18.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        font,
        color,
    );
    resp.clicked()
}

fn cat_node(ui: &mut egui::Ui, expanded: bool, selected: bool, label: &str, count: usize) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 5.0, T_ACCENT_SOFT);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, T_SURFACE);
    }
    let chev = if expanded { "v" } else { ">" };
    ui.painter().text(
        egui::pos2(rect.min.x + 16.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        chev,
        egui::FontId::proportional(10.0),
        T_MUTED,
    );
    let color = if selected || resp.hovered() { T_TEXT } else { T_MUTED };
    let font = egui::FontId::proportional(12.0);
    let text = truncate_text(ui, label, &font, rect.width() - 78.0);
    ui.painter().text(
        egui::pos2(rect.min.x + 32.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        font,
        color,
    );
    if count > 0 {
        ui.painter().text(
            egui::pos2(rect.max.x - 9.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            count.to_string(),
            egui::FontId::proportional(10.5),
            T_MUTED,
        );
    }
    resp.clicked()
}

/// A channel row, with the response so the caller can scroll it into view.
fn side_chan(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 5.0, T_ACCENT_SOFT);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(2.5, rect.height())),
            1.0,
            T_ACCENT,
        );
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, T_SURFACE);
    }
    let color = if selected || resp.hovered() { T_TEXT } else { T_MUTED };
    let font = egui::FontId::proportional(12.0);
    let text = truncate_text(ui, label, &font, rect.width() - 50.0);
    ui.painter().text(
        egui::pos2(rect.min.x + 40.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        font,
        color,
    );
    resp
}

fn live_badge(ui: &mut egui::Ui, a: f32) {
    let font = egui::FontId::proportional(11.0);
    let text_w = ui
        .fonts(|f| f.layout_no_wrap("LIVE".to_string(), font.clone(), egui::Color32::WHITE))
        .size()
        .x;
    let pad = 7.0;
    let dot = 6.0;
    let gap = 5.0;
    let w = pad + dot + gap + text_w + pad;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 20.0), egui::Sense::hover());
    let bg = egui::Color32::from_rgba_unmultiplied(0xe9, 0x19, 0x16, (a * 235.0) as u8);
    ui.painter().rect_filled(rect, 4.0, bg);
    let white = egui::Color32::from_white_alpha((a * 255.0) as u8);
    ui.painter()
        .circle_filled(egui::pos2(rect.min.x + pad + dot / 2.0, rect.center().y), 3.0, white);
    ui.painter().text(
        egui::pos2(rect.min.x + pad + dot + gap, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "LIVE",
        font,
        white,
    );
}

fn paint_play(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let pts = vec![
        egui::pos2(r.min.x + r.width() * 0.18, r.min.y),
        egui::pos2(r.max.x, r.center().y),
        egui::pos2(r.min.x + r.width() * 0.18, r.max.y),
    ];
    p.add(egui::Shape::convex_polygon(pts, c, egui::Stroke::NONE));
}

fn paint_pause(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let w = r.width() * 0.3;
    p.rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(w, r.height())), 1.0, c);
    p.rect_filled(
        egui::Rect::from_min_size(egui::pos2(r.max.x - w, r.min.y), egui::vec2(w, r.height())),
        1.0,
        c,
    );
}

fn paint_speaker(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let body = egui::Rect::from_min_size(
        egui::pos2(r.min.x, r.center().y - r.height() * 0.18),
        egui::vec2(r.width() * 0.24, r.height() * 0.36),
    );
    p.rect_filled(body, 1.0, c);
    let pts = vec![
        egui::pos2(body.max.x, body.min.y),
        egui::pos2(r.min.x + r.width() * 0.56, r.min.y),
        egui::pos2(r.min.x + r.width() * 0.56, r.max.y),
        egui::pos2(body.max.x, body.max.y),
    ];
    p.add(egui::Shape::convex_polygon(pts, c, egui::Stroke::NONE));
    let cx = r.min.x + r.width() * 0.6;
    for rad in [0.2_f32, 0.32] {
        let rr = r.width() * rad;
        let mut line = Vec::new();
        for k in 0..=6 {
            let ang = -0.7 + 1.4 * (k as f32 / 6.0);
            line.push(egui::pos2(cx + rr * ang.cos(), r.center().y + rr * ang.sin()));
        }
        p.add(egui::Shape::line(line, egui::Stroke::new(1.5_f32, c)));
    }
}

fn paint_fullscreen(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let l = r.width().min(r.height()) * 0.34;
    let corners = [
        (r.min, 1.0_f32, 1.0_f32),
        (egui::pos2(r.max.x, r.min.y), -1.0_f32, 1.0_f32),
        (egui::pos2(r.min.x, r.max.y), 1.0_f32, -1.0_f32),
        (egui::pos2(r.max.x, r.max.y), -1.0_f32, -1.0_f32),
    ];
    for (c0, dx, dy) in corners {
        p.line_segment([c0, egui::pos2(c0.x + dx * l, c0.y)], egui::Stroke::new(1.6_f32, c));
        p.line_segment([c0, egui::pos2(c0.x, c0.y + dy * l)], egui::Stroke::new(1.6_f32, c));
    }
}

fn vlc_path() -> Option<String> {
    if let Ok(p) = std::env::var("EG_VLC") {
        if !p.is_empty() {
            return Some(p);
        }
    }
    for p in [
        r"C:\Program Files\VideoLAN\VLC\vlc.exe",
        r"C:\Program Files (x86)\VideoLAN\VLC\vlc.exe",
    ] {
        if std::path::Path::new(p).exists() {
            return Some(p.to_string());
        }
    }
    None
}

fn play_url(url: &str) -> Result<(), String> {
    let vlc = vlc_path().ok_or_else(|| "VLC not found. Install VLC or set EG_VLC to vlc.exe".to_string())?;
    std::process::Command::new(vlc)
        .arg("--one-instance")
        .arg("--no-video-title-show")
        .arg("--network-caching=1500")
        .arg(url)
        .spawn()
        .map_err(|e| format!("Could not launch VLC: {}", e))?;
    Ok(())
}

struct CatRow {
    kind: Kind,
    id: String,
    name: String,
}

#[derive(Default)]
struct Msg {
    logged_in: Option<(String, Vec<CatRow>)>,
    items: Option<Vec<Item>>,
    play: Option<String>,
    error: Option<String>,
    /// Set when the pre-play guard has something to say. The URL is still in
    /// `play`; the UI decides whether to hold it.
    guard: Option<GuardOutcome>,
}

/// Someone else is holding the connection.
#[derive(Clone)]
struct Conflict {
    /// Display name, "You (another device)", or empty when we cannot tell.
    who: String,
    stream: String,
    minutes: i64,
    max_connections: i64,
}

impl Conflict {
    fn is_unknown(&self) -> bool {
        self.who.is_empty()
    }
}

/// A resolved stream URL waiting on the user's answer to the guard.
struct PendingPlay {
    url: String,
    conflict: Conflict,
}

/// What the guard decided to do about the line being busy.
#[derive(Clone)]
enum GuardOutcome {
    /// Nothing worth saying - go ahead.
    Free,
    /// Stop and ask, because we know exactly who would be displaced.
    Warn(Conflict),
    /// Go ahead, but say something in the status bar.
    Proceed { note: String },
}

/// Everything the guard thread needs to ask "is the line free, and who has it".
struct GuardContext {
    presence_url: String,
    presence_token: String,
    profile: String,
    server: String,
    user: String,
    pass: String,
}

impl GuardContext {
    fn check(&self) -> GuardOutcome {
        // The panel is the authority on *whether* the line is busy, but its
        // counter is only a hint: it is slow to clear. Measured against a real
        // panel, active_cons stayed at 1 for over four minutes with nothing of
        // ours running at all.
        let counts = probe_panel(&self.server, &self.user, &self.pass).ok();
        let roster = presence::Presence::new(&self.presence_url, &self.presence_token)
            .roster()
            .ok();
        match counts {
            Some((active, max)) => decide_guard(active, max, roster.as_ref(), &self.profile),
            // We could not ask. Do not invent a warning out of a failed request.
            None => GuardOutcome::Free,
        }
    }
}

/// What we can see of the line, before any judgement.
fn probe_panel(
    server: &str,
    user: &str,
    pass: &str,
) -> Result<(i64, i64), String> {
    let client = Xtream::new(server, user, pass)?;
    let info = client.authenticate()?;
    Ok(presence::account_counts(&info))
}

/// Should we stop before taking the line? Pure, so it can be tested without a
/// panel or a window.
///
/// Deliberately narrow: only stop when we can *name* somebody we would displace.
/// A nonzero `active_cons` with nobody checked in is ambiguous - it may be a
/// watcher we cannot see, or the panel's counter lagging behind a stream that
/// already ended - and a modal firing on that ambiguity would nag the user every
/// single time they pressed play, for as long as the counter stuck.
fn decide_guard(
    active_cons: i64,
    max_connections: i64,
    roster: Option<&Roster>,
    my_profile: &str,
) -> GuardOutcome {
    if active_cons <= 0 {
        return GuardOutcome::Free;
    }

    let mine = roster.map(|r| r.mine(my_profile)).unwrap_or(false);
    let other = roster.and_then(|r| r.others(my_profile).into_iter().next().cloned());

    if let Some(w) = other {
        // A friend is on the line, so we know exactly who gets cut.
        return GuardOutcome::Warn(Conflict {
            who: w.name.clone(),
            stream: w.stream_name.clone(),
            minutes: w.watching_seconds / 60,
            max_connections,
        });
    }

    if mine {
        return GuardOutcome::Warn(Conflict {
            who: "You (another device)".to_string(),
            stream: String::new(),
            minutes: 0,
            max_connections,
        });
    }

    // A connection is counted but nobody of ours claims one. Take it, and say so
    // in the status bar rather than blocking on it.
    GuardOutcome::Proceed {
        note: format!(
            "Panel reports {}/{} in use with nobody checked in - taking the line",
            active_cons,
            if max_connections > 0 {
                max_connections.to_string()
            } else {
                "?".to_string()
            }
        ),
    }
}

struct App {
    server: String,
    user: String,
    pass: String,
    live_format: String,
    status: String,
    cats: Vec<CatRow>,
    items: Vec<Item>,
    kind: Kind,
    cat: Option<String>,
    selected: Option<usize>,
    rx: Option<Receiver<Msg>>,
    client: Option<Xtream>,
    side_width: Option<f32>,
    width_applied: bool,
    player: video::Player,
    autostart: bool,
    autoplay: Option<String>,
    fullscreen: bool,
    muted: bool,
    pre_mute_vol: i32,
    open_live: bool,
    open_vod: bool,
    open_series: bool,
    open_cats: std::collections::HashSet<String>,
    now_playing: String,
    /// Item id of the current stream, so presence can report *what* is on.
    playing_id: String,
    show_settings: bool,
    /// Two-step guard in front of the destructive reset.
    confirm_reset: bool,
    wizard: bool,
    vol_reapply: u32,
    prev_lbtn: bool,
    last_click: f64,
    bright: f32,
    contrast: f32,
    gamma: f32,

    // --- presence (who else is watching on the shared line) ---
    /// Display name of whoever is at this install, e.g. "Peter". Empty means
    /// not chosen yet, and presence stays off until it is.
    profile: String,
    device_id: String,
    presence_url: String,
    presence_token: String,
    /// id -> display name, refreshed from the worker so adding a friend does
    /// not need a new exe.
    profiles: Vec<(String, String)>,
    shared: Arc<Mutex<Shared>>,
    roster: Arc<Mutex<Roster>>,
    presence_thread: bool,
    /// Profile as last reported to the worker, so a change can be pushed.
    reported_profile: String,
    /// A stream is resolved but the guard is waiting on the user.
    pending_play: Option<PendingPlay>,
    /// When the current stream actually started, for drop detection.
    play_started_at: i64,
    /// Distinguishes a deliberate pause from a stream that died.
    paused_by_user: bool,
    /// Non-zero while a drop is being attributed to whoever took the line.
    drop_at: i64,
    /// What was on disk at startup, so env-overridden settings can be kept out
    /// of the file when we save.
    file_cfg: Config,
    env: EnvOverrides,
    /// The profile as last written to disk, so a change is saved once.
    persisted_profile: String,
    /// Volume as last written to disk, plus when to write a pending change.
    persisted_volume: i32,
    volume_save_at: i64,
    /// Picture settings remembered per stream.
    pictures: Vec<Picture>,
    picture_dirty: bool,
    picture_save_at: i64,
    /// A channel the TV guide asked for, waiting until we can play it.
    launch_request: Option<launch::PlayRequest>,
    request_poll_at: i64,
    /// A launched channel's category, applied once the play is in flight - the
    /// app has a single receiver for background work, so the two steps have to
    /// take turns.
    pending_sidebar: Option<(String, String)>,
    /// The channel to highlight once its category's list has loaded.
    pending_select: Option<String>,
    /// One-shot: scroll the highlighted row into view.
    scroll_to_selected: bool,
}

fn clone_client(c: &Option<Xtream>) -> Option<Xtream> {
    c.as_ref().map(|c| Xtream {
        scheme: c.scheme.clone(),
        host: c.host.clone(),
        port: c.port,
        user: c.user.clone(),
        pass: c.pass.clone(),
        timeout: c.timeout,
    })
}

/// Which settings arrived from the environment this run.
///
/// These are per-run overrides and must never be written back to config.txt,
/// or an override quietly becomes permanent. Every save path has to respect
/// this, not just startup: the first version only guarded the device-id
/// bootstrap, so a successful login wrote EG_PRESENCE_URL and the token into
/// the file anyway.
#[derive(Default, Clone, Copy)]
struct EnvOverrides {
    server: bool,
    user: bool,
    pass: bool,
    format: bool,
    profile: bool,
    presence_url: bool,
    presence_token: bool,
}

impl Default for App {
    fn default() -> Self {
        let mut cfg = load_config();

        let env_val = |key: &str| std::env::var(key).ok().filter(|s| !s.is_empty());
        let user_env = env_val("EG_USER");
        let pass_env = env_val("EG_PASS");
        let server_env = env_val("EG_SERVER");
        let format_env = env_val("EG_FORMAT");
        let profile_env = env_val("EG_PROFILE");
        let url_env = env_val("EG_PRESENCE_URL");
        let token_env = env_val("EG_PRESENCE_TOKEN");

        let overrides = EnvOverrides {
            server: server_env.is_some(),
            user: user_env.is_some(),
            pass: pass_env.is_some(),
            format: format_env.is_some(),
            profile: profile_env.is_some(),
            presence_url: url_env.is_some(),
            presence_token: token_env.is_some(),
        };

        let user = user_env.unwrap_or_else(|| cfg.user.clone());
        let pass = pass_env.unwrap_or_else(|| cfg.pass.clone());
        let server = server_env.unwrap_or_else(|| {
            if cfg.server.is_empty() {
                DEFAULT_SERVER.to_string()
            } else {
                cfg.server.clone()
            }
        });
        let live_format = format_env.unwrap_or_else(|| {
            if cfg.format.is_empty() {
                "ts".to_string()
            } else {
                cfg.format.clone()
            }
        });
        let profile = profile_env.unwrap_or_else(|| cfg.profile.clone());
        let presence_url = url_env.unwrap_or_else(|| cfg.presence_url.clone());
        let presence_token = token_env.unwrap_or_else(|| cfg.presence_token.clone());

        // A device id must survive restarts: it is the KV key suffix, so a new
        // one every launch would leave ghost entries in the roster for a full
        // TTL. Generated once, then persisted.
        let device_id = if cfg.device_id.is_empty() {
            let id = presence::new_device_id();
            cfg.device_id = id.clone();
            save_config(&cfg);
            id
        } else {
            cfg.device_id.clone()
        };
        let persisted_profile = cfg.profile.clone();
        let file_cfg = cfg;
        let start_pictures = file_cfg.pictures.clone();

        // The player starts at a hardcoded 100, so apply the remembered level
        // before anything can play. Without this, every fresh instance is at
        // full volume until you touch the slider.
        let mut player = video::Player::new();
        let start_volume = file_cfg.volume.max(1);
        player.set_volume(start_volume);

        let autostart = !user.is_empty() && !pass.is_empty();

        let shared = Arc::new(Mutex::new(Shared {
            url: presence_url.clone(),
            token: presence_token.clone(),
            profile: profile.clone(),
            device_id: device_id.clone(),
            device: presence::device_name(),
            server: server.clone(),
            user: user.clone(),
            pass: pass.clone(),
            ..Default::default()
        }));

        App {
            server,
            user,
            pass,
            live_format,
            status: "Not connected".to_string(),
            cats: Vec::new(),
            items: Vec::new(),
            kind: Kind::Live,
            cat: None,
            selected: None,
            rx: None,
            client: None,
            side_width: None,
            width_applied: false,
            player,
            autostart,
            autoplay: std::env::var("EG_AUTOPLAY").ok().filter(|s| !s.is_empty()),
            fullscreen: false,
            muted: false,
            pre_mute_vol: start_volume,
            open_live: true,
            open_vod: false,
            open_series: false,
            open_cats: std::collections::HashSet::new(),
            now_playing: String::new(),
            playing_id: String::new(),
            show_settings: std::env::var("EG_SETTINGS").is_ok(),
            confirm_reset: false,
            wizard: !autostart,
            vol_reapply: 0,
            prev_lbtn: false,
            last_click: 0.0,
            bright: 1.0,
            contrast: 1.0,
            gamma: 1.0,
            profile,
            device_id,
            presence_url,
            presence_token,
            profiles: presence::DEFAULT_PROFILES
                .iter()
                .map(|(id, name)| (id.to_string(), name.to_string()))
                .collect(),
            shared,
            roster: Arc::new(Mutex::new(Roster::default())),
            presence_thread: false,
            reported_profile: String::new(),
            pending_play: None,
            play_started_at: 0,
            paused_by_user: false,
            drop_at: 0,
            file_cfg,
            env: overrides,
            persisted_profile,
            persisted_volume: start_volume,
            volume_save_at: 0,
            pictures: start_pictures,
            picture_dirty: false,
            picture_save_at: 0,
            // Whatever the guide page asked for before we were even running.
            launch_request: startup_request().lock().ok().and_then(|mut s| s.take()),
            request_poll_at: 0,
            pending_sidebar: None,
            pending_select: None,
            scroll_to_selected: false,
        }
    }
}

impl App {
    fn spawn_login(&mut self) {
        let server = self.server.clone();
        let user = self.user.clone();
        let pass = self.pass.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.status = "Connecting...".to_string();
        thread::spawn(move || {
            let mut msg = Msg::default();
            let result = Xtream::new(&server, &user, &pass).and_then(|c| {
                let ui = c.authenticate()?;
                let mut rows = Vec::new();
                for (kind, cats) in [
                    (Kind::Live, c.live_categories()),
                    (Kind::Vod, c.vod_categories()),
                    (Kind::Series, c.series_categories()),
                ] {
                    if let Ok(list) = cats {
                        for cat in list {
                            rows.push(CatRow { kind, id: cat.id, name: cat.name });
                        }
                    }
                }
                Ok((ui, rows))
            });
            match result {
                Ok((ui, rows)) => {
                    let info = format!(
                        "{} | status={} | exp={} | max conn={}",
                        ui.get("username").and_then(|v| v.as_str()).unwrap_or(&user),
                        ui.get("status").and_then(|v| v.as_str()).unwrap_or("?"),
                        ui.get("exp_date").and_then(|v| v.as_str()).unwrap_or("n/a"),
                        ui.get("max_connections").and_then(|v| v.as_str()).unwrap_or("?")
                    );
                    msg.logged_in = Some((info, rows));
                }
                Err(e) => msg.error = Some(e),
            }
            let _ = tx.send(msg);
        });
    }

    fn spawn_items(&mut self, kind: Kind, cat: Option<String>) {
        let client = match clone_client(&self.client) {
            Some(c) => c,
            None => return,
        };
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.status = format!("Loading {}...", kind.label());
        thread::spawn(move || {
            let mut msg = Msg::default();
            match client.category_items(kind, cat.as_deref()) {
                Ok(items) => msg.items = Some(items),
                Err(e) => msg.error = Some(e),
            }
            let _ = tx.send(msg);
        });
    }

    fn spawn_play(&mut self, kind: Kind, item: Item) {
        let client = match clone_client(&self.client) {
            Some(c) => c,
            None => return,
        };
        let fmt = self.live_format.clone();
        self.now_playing = item.name.clone();
        self.playing_id = item.id.clone();
        self.apply_saved_picture(&item.id);

        // Only guard when we are not already holding the connection: once a
        // stream is up, zapping channels cannot displace anybody.
        let already_watching = self.shared.lock().map(|g| g.active).unwrap_or(false);
        let guard = if !already_watching && self.presence_ready() {
            Some(GuardContext {
                presence_url: self.presence_url.clone(),
                presence_token: self.presence_token.clone(),
                profile: self.profile_id(),
                server: self.server.clone(),
                user: self.user.clone(),
                pass: self.pass.clone(),
            })
        } else {
            None
        };

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.status = format!("Preparing {}...", item.name);
        thread::spawn(move || {
            let mut msg = Msg::default();
            match client.play_url(kind, &item, &fmt) {
                Ok(url) => {
                    if let Some(ctx) = guard {
                        msg.guard = Some(ctx.check());
                    }
                    msg.play = Some(url);
                }
                Err(e) => msg.error = Some(e),
            }
            let _ = tx.send(msg);
        });
    }

    fn pump(&mut self) {
        let received = match &self.rx {
            Some(rx) => rx.try_recv().ok(),
            None => None,
        };
        if let Some(msg) = received {
            self.rx = None;
            let mut load_live = false;
            if let Some((info, rows)) = msg.logged_in {
                self.status = info;
                self.wizard = false;
                save_config(&self.config_snapshot());
                self.cats = rows;
                self.side_width = None;
                self.width_applied = false;
                if let Ok(c) = Xtream::new(&self.server, &self.user, &self.pass) {
                    self.client = Some(c);
                }
                self.kind = Kind::Live;
                load_live = true;
            }
            if let Some(items) = msg.items {
                self.status = format!("{} item(s)", items.len());
                self.items = items;
                self.selected = None;
                // A channel launched from the guide gets highlighted, and the
                // list scrolled to it - a category can hold eighty rows.
                if let Some(id) = self.pending_select.take() {
                    if let Some(i) = self.items.iter().position(|it| it.id == id) {
                        self.selected = Some(i);
                        self.scroll_to_selected = true;
                    }
                }
                if let Some(id) = self.autoplay.take() {
                    if let Some(i) = self.items.iter().position(|it| it.id == id) {
                        let kind = self.kind;
                        let item = self.items[i].clone();
                        self.spawn_play(kind, item);
                    }
                }
            }
            if let Some(url) = msg.play {
                match msg.guard {
                    // Somebody we can name would be displaced: hold the URL and ask.
                    Some(GuardOutcome::Warn(conflict)) => {
                        self.status = format!("{} is on the line", describe_conflict(&conflict));
                        self.pending_play = Some(PendingPlay { url, conflict });
                    }
                    // Nobody to displace - play, but say what the panel claimed.
                    Some(GuardOutcome::Proceed { note }) => {
                        self.start_stream(url);
                        self.status = note;
                    }
                    _ => self.start_stream(url),
                }
            }
            if let Some(e) = msg.error {
                self.status = e;
            }
            if load_live {
                self.spawn_items(Kind::Live, None);
            }
        }
    }

    /* --------------------------------------------------------- presence ---- */

    /// The on-disk representation, built from current UI state.
    ///
    /// Anything that came from the environment keeps the value that was
    /// already on disk, so an override never becomes permanent.
    fn config_snapshot(&self) -> Config {
        let keep = |from_env: bool, live: &str, on_disk: &str| -> String {
            if from_env {
                on_disk.to_string()
            } else {
                live.to_string()
            }
        };
        Config {
            server: keep(self.env.server, &self.server, &self.file_cfg.server),
            user: keep(self.env.user, &self.user, &self.file_cfg.user),
            pass: keep(self.env.pass, &self.pass, &self.file_cfg.pass),
            format: keep(self.env.format, &self.live_format, &self.file_cfg.format),
            profile: keep(self.env.profile, &self.profile, &self.file_cfg.profile),
            device_id: self.device_id.clone(),
            // The audible level, not 0: if you mute and then quit, the next
            // launch should not start silent with no obvious explanation.
            volume: self.live_volume(),
            pictures: self.pictures.clone(),
            presence_url: keep(
                self.env.presence_url,
                &self.presence_url,
                &self.file_cfg.presence_url,
            ),
            presence_token: keep(
                self.env.presence_token,
                &self.presence_token,
                &self.file_cfg.presence_token,
            ),
        }
    }

    /// The lowercase id the worker knows us by, e.g. "peter".
    fn profile_id(&self) -> String {
        self.profiles
            .iter()
            .find(|(_, name)| name.eq_ignore_ascii_case(&self.profile))
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| self.profile.to_lowercase())
    }

    fn presence_ready(&self) -> bool {
        !self.presence_url.is_empty()
            && !self.presence_token.is_empty()
            && !self.profile.is_empty()
            && !self.device_id.is_empty()
    }

    fn presence_roster(&self) -> Roster {
        self.roster.lock().map(|r| r.clone()).unwrap_or_default()
    }

    fn ensure_presence_thread(&mut self) {
        if self.presence_thread {
            return;
        }
        self.presence_thread = true;
        presence::spawn(Arc::clone(&self.shared), Arc::clone(&self.roster));
    }

    /// Push identity and playback state down to the presence thread. Cheap
    /// enough to run every frame; it is one mutex lock and a few clones.
    fn presence_sync(&mut self) {
        // State comes from what the user asked for, not from polling libVLC:
        // is_playing() reports not-playing while rebuffering, so a single flaky
        // reading used to show "paused" on the dashboard for up to 90 seconds
        // while the picture was fine.
        let state = if self.paused_by_user { "paused" } else { "playing" };
        let id = self.profile_id();

        // Picking a name is a choice worth keeping. Without this, choosing it
        // from the top bar leaves presence working for that session only - and
        // the file is otherwise only written on a successful login or when the
        // settings dialog is used, neither of which a pre-configured install
        // goes through.
        if self.profile != self.persisted_profile {
            self.persisted_profile = self.profile.clone();
            save_config(&self.config_snapshot());
        }

        // The worker owns the friend group, so its list wins. Without this the
        // dropdown was always the built-in four and adding a friend would have
        // needed a new exe, which is not what the README claimed.
        let worker_profiles = self
            .roster
            .lock()
            .ok()
            .map(|r| r.profiles.clone())
            .unwrap_or_default();
        if !worker_profiles.is_empty() && worker_profiles != self.profiles {
            self.profiles = worker_profiles;
        }

        if !self.reported_profile.is_empty() && self.reported_profile != id {
            if let Ok(mut g) = self.shared.lock() {
                g.stale_profile = self.reported_profile.clone();
            }
        }
        if let Ok(mut g) = self.shared.lock() {
            g.enabled = self.presence_ready();
            g.url = self.presence_url.clone();
            g.token = self.presence_token.clone();
            g.profile = id.clone();
            g.device_id = self.device_id.clone();
            g.device = presence::device_name();
            g.state = state.to_string();
            g.server = self.server.clone();
            g.user = self.user.clone();
            g.pass = self.pass.clone();
        }
        self.reported_profile = id;
    }

    /// A stream just started, so this install now holds a panel connection.
    fn presence_start(&mut self) {
        let kind = match self.kind {
            Kind::Live => "live",
            Kind::Vod => "vod",
            Kind::Series => "series",
        };
        if let Ok(mut g) = self.shared.lock() {
            // Zapping between channels keeps the original start time so the
            // "watching for" clock does not reset on every channel change.
            if !g.active {
                g.since = presence::now_ms();
            }
            g.active = true;
            g.state = "playing".to_string();
            g.stream_kind = kind.to_string();
            g.stream_id = self.playing_id.clone();
            g.stream_name = self.now_playing.clone();
            // Re-read the panel's counters now: the last reading predates this
            // stream, so it would still say nobody is watching.
            g.force_account = true;
        }
    }

    /// "Watching as" dropdown - shared by the wizard, the top bar and settings.
    fn profile_combo(&mut self, ui: &mut egui::Ui, id: &str, width: f32) {
        let list = self.profiles.clone();
        let selected = if self.profile.is_empty() {
            "Select…".to_string()
        } else {
            self.profile.clone()
        };
        egui::ComboBox::from_id_source(id)
            .width(width)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for (_, name) in list.iter() {
                    ui.selectable_value(&mut self.profile, name.clone(), name);
                }
            });
    }

    fn dashboard_url(&self) -> String {
        if self.presence_url.is_empty() {
            return String::new();
        }
        format!(
            "{}/?key={}",
            self.presence_url.trim_end_matches('/'),
            self.presence_token
        )
    }

    /// The presence pill in the top bar.
    fn presence_pill_ui(&mut self, ui: &mut egui::Ui) {
        if !self.presence_ready() {
            let hint = if self.presence_url.is_empty() || self.presence_token.is_empty() {
                "Presence not set up"
            } else {
                "Pick who's watching"
            };
            pill_resp(ui, T_MUTED, hint).on_hover_text(
                "Set presence_url, presence_token and profile in the settings window \
                 (the gear, top right) - or in %APPDATA%\\EagleGlass\\config.txt.",
            );
            return;
        }

        let roster = self.presence_roster();

        if let Some(err) = &roster.error {
            pill_resp(ui, T_DANGER, "Presence unreachable").on_hover_text(err.clone());
            return;
        }
        if !roster.fresh() {
            pill_resp(ui, T_MUTED, "Checking who's watching…");
            return;
        }

        let me = self.profile_id();
        let (color, text) = pill_state(&roster, &me);
        let others: Vec<presence::Watcher> = roster.others(&me).into_iter().cloned().collect();
        let mine = roster.mine(&me);
        let maxc = roster.max_connections;
        let _ = (mine, maxc, others);

        let mut hover = String::new();
        if roster.unaccounted > 0 {
            hover.push_str(&format!(
                "The panel reports {} live connection{} but only {} of our device{} {} checked in — someone may be watching without us.\n\n",
                roster.active_cons,
                if roster.active_cons == 1 { "" } else { "s" },
                roster.watchers.len(),
                if roster.watchers.len() == 1 { "" } else { "s" },
                if roster.watchers.len() == 1 { "is" } else { "are" },
            ));
        }
        if roster.watchers.is_empty() {
            hover.push_str("Nobody is checked in.\n");
        }
        for w in &roster.watchers {
            let who = if w.profile == me {
                format!("{} (you)", w.name)
            } else {
                w.name.clone()
            };
            let chan = if w.stream_name.is_empty() {
                "no stream"
            } else {
                w.stream_name.as_str()
            };
            let mut line = format!("{} · {} · {}", who, chan, presence::human_duration(w.watching_seconds));
            if w.state == "paused" {
                line.push_str(" · paused");
            }
            // A stale heartbeat means their client is struggling, not that they
            // are gone - worth showing rather than silently hiding.
            if w.age_seconds > 90 {
                line.push_str(&format!(" · last seen {}s ago", w.age_seconds));
            }
            line.push_str(&format!(" · on {}", w.device));
            hover.push_str(&line);
            hover.push('\n');
        }
        if roster.max_connections > 0 {
            hover.push_str(&format!(
                "\nConnections: {}/{} in use",
                roster.active_cons, roster.max_connections
            ));
        }
        hover.push_str("\n\nClick to open the dashboard.");

        let url = self.dashboard_url();
        let resp = pill_resp(ui, color, &text).on_hover_text(hover);
        let resp = ui.interact(resp.rect, resp.id, egui::Sense::click());
        if resp.clicked() && !url.is_empty() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    }

    /// Top bar: who else is on the line. Identity itself is deliberately *not*
    /// here - it is set once in the wizard and changed only from Settings, since
    /// there is no reason to switch who you are mid-session.
    fn presence_top_ui(&mut self, ui: &mut egui::Ui) {
        self.presence_pill_ui(ui);
    }

    /// Start playback for real, once any guard has been satisfied (or skipped).
    fn start_stream(&mut self, url: String) {
        match self.player.play(&url) {
            Ok(_) => {
                self.status = "Playing".to_string();
                self.vol_reapply = 60;
                self.paused_by_user = false;
                self.drop_at = 0;
                self.play_started_at = presence::now_ms();
                // Only now is there genuinely a connection on the panel, so
                // this is the honest moment to announce.
                self.presence_start();
            }
            Err(e) => self.status = e,
        }
    }

    /// Pause is a deliberate act, so it must not be mistaken for a stream that
    /// died. Both call sites go through here.
    fn toggle_pause(&mut self) {
        self.player.pause_toggle();
        self.paused_by_user = !self.paused_by_user;
        self.play_started_at = presence::now_ms();
    }

    /// Pre-play guard: warn before taking a line somebody else is on.
    fn guard_dialog(&mut self, ctx: &egui::Context) {
        let conflict = match self.pending_play.as_ref() {
            Some(p) => p.conflict.clone(),
            None => return,
        };
        let mut take_it = false;
        let mut cancel = false;
        let mut open = true;
        let slots = conflict.max_connections.max(1);

        egui::Window::new("Line in use")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_width(360.0);
                ui.add_space(2.0);
                if conflict.is_unknown() {
                    ui.label(
                        egui::RichText::new("Someone is watching that we can't identify")
                            .size(15.0)
                            .strong(),
                    );
                } else if conflict.who.starts_with("You ") {
                    ui.label(
                        egui::RichText::new("You're already watching on another device")
                            .size(15.0)
                            .strong(),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(format!("{} is watching", conflict.who))
                            .size(15.0)
                            .strong(),
                    );
                }

                if !conflict.stream.is_empty() {
                    ui.add_space(4.0);
                    let mut line = conflict.stream.clone();
                    if conflict.minutes > 0 {
                        line.push_str(&format!(" · {}m in", conflict.minutes));
                    }
                    ui.label(egui::RichText::new(line).size(12.0).color(T_MUTED));
                }

                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(format!(
                        "This line allows {} connection{}. Starting will kick them off.",
                        slots,
                        if slots == 1 { "" } else { "s" }
                    ))
                    .size(11.5)
                    .color(T_DANGER),
                );

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui.add_sized([150.0, 28.0], egui::Button::new("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui
                        .add_sized([150.0, 28.0], egui::Button::new("Take the line"))
                        .clicked()
                    {
                        take_it = true;
                    }
                });
            });

        if cancel || !open {
            self.pending_play = None;
            self.status = "Cancelled".to_string();
        } else if take_it {
            if let Some(p) = self.pending_play.take() {
                self.status = format!("Taking the line from {}", describe_conflict(&p.conflict));
                self.start_stream(p.url);
            }
        }
    }

    /// Did somebody take the line off us mid-match?
    fn check_stream_drop(&mut self) {
        let active = self.shared.lock().map(|g| g.active).unwrap_or(false);
        if !active || self.paused_by_user || self.drop_at == 0 {
            return;
        }
        let now = presence::now_ms();

        // Ask for a fresh roster, then give it a few seconds to arrive before
        // giving up and reporting a plain drop.
        if let Ok(mut g) = self.shared.lock() {
            g.force_poll = true;
        }
        if now - self.drop_at > 8000 {
            self.drop_at = 0;
            return;
        }

        let me = self.profile_id();
        let roster = self.presence_roster();
        let others: Vec<presence::Watcher> = roster.others(&me).into_iter().cloned().collect();
        if let Some(w) = others.first() {
            if roster.max_connections == 1 {
                self.status = if w.stream_name.is_empty() {
                    format!("{} took the line — your stream was cut", w.name)
                } else {
                    format!("{} took the line — now on {}", w.name, clip(&w.stream_name, 40))
                };
                self.drop_at = 0;
            }
        }
    }

    /// Notice that our stream stopped without the user asking.
    fn detect_drop(&mut self) {
        let active = self.shared.lock().map(|g| g.active).unwrap_or(false);
        if !active || self.paused_by_user || self.drop_at != 0 {
            return;
        }
        if self.player.is_playing() {
            return;
        }
        // libVLC reports "not playing" while buffering a fresh stream.
        if self.play_started_at > 0 && presence::now_ms() - self.play_started_at < 6000 {
            return;
        }
        // We no longer hold a connection, so stop claiming one.
        if let Ok(mut g) = self.shared.lock() {
            g.active = false;
        }
        self.drop_at = presence::now_ms().max(1);
        self.status = "Stream stopped".to_string();
    }

    fn set_fullscreen(&mut self, ctx: &egui::Context, on: bool) {
        self.fullscreen = on;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
    }
    fn toggle_mute(&mut self) {
        if self.muted {
            self.muted = false;
            self.player.set_volume(self.pre_mute_vol.max(1));
        } else {
            self.pre_mute_vol = self.player.volume().max(1);
            self.muted = true;
            self.player.set_volume(0);
        }
    }

    fn wizard_ui(&mut self, ctx: &egui::Context) {
        let connecting = self.rx.is_some();
        let submit = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let mut do_login = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.16).max(24.0));
                egui::Frame::none()
                    .fill(T_SURFACE)
                    .rounding(egui::Rounding::same(12.0))
                    .inner_margin(egui::Margin::same(22.0))
                    .show(ui, |ui| {
                        ui.set_max_width(320.0);
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("Welcome to Eagle Glass").size(20.0).strong(),
                            );
                            ui.add_space(3.0);
                            ui.label(
                                egui::RichText::new("Enter your Xtream panel details to get started.")
                                    .size(11.5)
                                    .color(T_MUTED),
                            );
                            ui.add_space(16.0);
                            let m = egui::Margin::symmetric(6.0, 5.0);
                            ui.label(egui::RichText::new("Server").size(10.5).color(T_MUTED));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.server)
                                    .desired_width(320.0)
                                    .margin(m)
                                    .hint_text("http://host:port"),
                            );
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new("Username").size(10.5).color(T_MUTED));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.user)
                                    .desired_width(320.0)
                                    .margin(m),
                            );
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new("Password").size(10.5).color(T_MUTED));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.pass)
                                    .desired_width(320.0)
                                    .password(true)
                                    .margin(m),
                            );
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new("Watching as").size(10.5).color(T_MUTED));
                            self.profile_combo(ui, "profile_wizard", 320.0);
                            ui.add_space(16.0);
                            let picked = !self.profile.is_empty();
                            let label = if connecting { "Connecting..." } else { "Connect" };
                            ui.add_enabled_ui(picked, |ui| {
                                if ui.add_sized([320.0, 32.0], egui::Button::new(label)).clicked()
                                    && !connecting
                                {
                                    do_login = true;
                                }
                            });
                            if !picked {
                                ui.add_space(8.0);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(
                                            "Choose who's watching so the others can see if you're on.",
                                        )
                                        .size(11.0)
                                        .color(T_MUTED),
                                    )
                                    .wrap(),
                                );
                            }
                            if !self.status.is_empty()
                                && self.status != "Not connected"
                                && !self.status.starts_with("Connecting")
                            {
                                ui.add_space(10.0);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(self.status.clone())
                                            .size(11.0)
                                            .color(T_DANGER),
                                    )
                                    .wrap(),
                                );
                            }
                        });
                    });
            });
        });
        if do_login || (submit && !connecting && !self.profile.is_empty()) {
            self.spawn_login();
        }
    }

    fn apply_bounds(&mut self, ctx: &egui::Context, rect: egui::Rect) {
        let ppp = ctx.pixels_per_point();
        self.player
            .set_bounds(rect.min.x * ppp, rect.min.y * ppp, rect.width() * ppp, rect.height() * ppp);
    }

    /// Wipe the local settings file and go back to first-run state.
    ///
    /// Everything in the panel is set once, so there has to be a way to start
    /// over without hunting for %APPDATA%. The presence entry is cleared first,
    /// while the credentials to clear it are still in hand, so the others do not
    /// keep seeing a watcher that no longer exists.
    fn reset_app(&mut self) {
        let snap = self.shared.lock().map(|g| g.clone()).unwrap_or_default();
        if snap.enabled && snap.active && !snap.url.is_empty() {
            let _ = presence::Presence::new(&snap.url, &snap.token).idle(&snap.profile, &snap.device_id);
        }

        if let Some(p) = config_path() {
            let _ = std::fs::remove_file(p);
        }

        self.server = DEFAULT_SERVER.to_string();
        self.user.clear();
        self.pass.clear();
        self.live_format = "ts".to_string();
        self.profile.clear();
        self.presence_url.clear();
        self.presence_token.clear();
        self.device_id = presence::new_device_id();
        self.file_cfg = Config::default();
        self.persisted_profile.clear();
        self.reported_profile.clear();

        self.rx = None;
        self.client = None;
        self.cats.clear();
        self.items.clear();
        self.cat = None;
        self.selected = None;
        self.now_playing.clear();
        self.playing_id.clear();
        self.pending_play = None;
        self.drop_at = 0;
        self.paused_by_user = false;
        self.status = "Not connected".to_string();

        self.bright = 1.0;
        self.contrast = 1.0;
        self.gamma = 1.0;
        self.player.stop();

        if let Ok(mut g) = self.shared.lock() {
            g.active = false;
            g.enabled = false;
            g.url.clear();
            g.token.clear();
            g.profile.clear();
            g.stream_name.clear();
            g.stream_id.clear();
            g.active_cons = 0;
            g.max_connections = 0;
        }

        self.wizard = true;
        self.show_settings = false;
        self.confirm_reset = false;
        // Volume survives a reset on purpose: wiping credentials should not make
        // the next stream deafening.
        self.persisted_volume = self.live_volume();
        self.volume_save_at = 0;
    }

    /// The volume that should be remembered: the audible one, so muting and
    /// quitting does not persist a silent start.
    fn live_volume(&self) -> i32 {
        if self.muted {
            self.pre_mute_vol.max(1)
        } else {
            self.player.volume().max(1)
        }
    }

    /// Remember the volume, so a fresh instance does not blast the speakers.
    ///
    /// Debounced: the slider reports a change on every pixel of a drag, and this
    /// should write the file once the value settles rather than forty times.
    fn persist_volume(&mut self, ctx: &egui::Context) {
        let live = self.live_volume();
        if live == self.persisted_volume {
            self.volume_save_at = 0;
            return;
        }
        let now = presence::now_ms();
        if self.volume_save_at == 0 {
            self.volume_save_at = now + 700;
        }
        if now >= self.volume_save_at {
            self.persisted_volume = live;
            self.volume_save_at = 0;
            save_config(&self.config_snapshot());
        } else {
            // Nothing else may repaint while the app sits idle, so make sure we
            // come back to actually perform the write.
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Apply whatever was remembered for a stream, or the untouched defaults if
    /// it has never been tuned. Called when a channel is picked, so the sliders
    /// are already showing that channel's settings before the picture appears.
    fn apply_saved_picture(&mut self, id: &str) {
        let (b, c, g) = picture_for(&self.pictures, id);
        if (self.bright, self.contrast, self.gamma) != (b, c, g) {
            self.bright = b;
            self.contrast = c;
            self.gamma = g;
            self.player.set_picture(b, c, g);
        }
    }

    /// Remember the picture for the stream that is on.
    fn record_picture(&mut self) {
        if self.playing_id.is_empty() {
            return;
        }
        upsert_picture(
            &mut self.pictures,
            &self.playing_id,
            &self.now_playing,
            self.bright,
            self.contrast,
            self.gamma,
        );
        save_config(&self.config_snapshot());
        if !self.now_playing.is_empty() {
            self.status = format!("Picture saved for {}", self.now_playing);
        }
    }

    /// Write picture tweaks once they settle. A slider reports a change on every
    /// pixel of a drag, and this must not be one file write per pixel.
    fn persist_picture(&mut self, ctx: &egui::Context) {
        if !self.picture_dirty {
            return;
        }
        let now = presence::now_ms();
        if self.picture_save_at == 0 {
            self.picture_save_at = now + 700;
        }
        if now >= self.picture_save_at {
            self.picture_save_at = 0;
            self.picture_dirty = false;
            self.record_picture();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Pick up a request from the guide page, and act on it once we can.
    ///
    /// Polled rather than event-driven because the alternative on Windows is
    /// subclassing the window to receive a message from the second instance, and
    /// a stat check once a second is a much smaller thing to own.
    fn poll_launch_request(&mut self, ctx: &egui::Context) {
        let now = presence::now_ms();
        if now - self.request_poll_at >= 1000 {
            self.request_poll_at = now;
            if let Some(req) = launch::take() {
                match req {
                    launch::Request::Focus => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    launch::Request::Play(p) => self.launch_request = Some(p),
                }
            }
        }

        let Some(req) = self.launch_request.clone() else {
            return;
        };
        // Before the panel login there is nothing to resolve an id against, so
        // the request waits rather than being dropped.
        if self.client.is_none() {
            self.status = format!("Open {} once the panel login is done", req.name);
            return;
        }
        self.launch_request = None;
        let item = Item {
            id: req.id.clone(),
            name: req.name.clone(),
            ext: String::new(),
            cat: String::new(),
        };
        self.status = format!("Opening {} from the guide", item.name);
        // Play first - that is what the click asked for - and follow it in the
        // sidebar afterwards.
        self.spawn_play(Kind::Live, item);
        if let Some(cat) = req.cat.clone() {
            self.pending_sidebar = Some((cat, req.id.clone()));
        } else if self.kind == Kind::Live {
            // No category in the link, but if it is already on screen we can
            // still highlight it when the list settles.
            if self.items.iter().any(|it| it.id == req.id) {
                self.pending_select = Some(req.id.clone());
            }
        }
    }

    /// Show the launched channel in the sidebar: open its category and highlight
    /// the row. The sidebar is built from the complete live list, so that list is
    /// what gets loaded - fetching only the linked category would leave every
    /// other category empty and make switching impossible.
    fn follow_launch_in_sidebar(&mut self) {
        if self.rx.is_some() || self.pending_sidebar.is_none() {
            return;
        }
        let Some((cat, id)) = self.pending_sidebar.take() else {
            return;
        };
        // The category has to exist in the catalogue we were given.
        let known = self
            .cats
            .iter()
            .any(|c| c.kind == Kind::Live && c.id == cat);
        if !known {
            return;
        }
        // One step at a time, because only one background job can be in flight.
        let have_live = self.kind == Kind::Live && !self.items.is_empty();
        self.kind = Kind::Live;
        self.open_live = true;
        self.open_cats.insert(cat.clone());
        self.cat = Some(cat);
        self.width_applied = false;
        if have_live {
            if let Some(i) = self.items.iter().position(|it| it.id == id) {
                self.selected = Some(i);
                self.scroll_to_selected = true;
            }
        } else {
            self.pending_select = Some(id);
            self.spawn_items(Kind::Live, None);
        }
    }

    /// The settings form itself, laid out in the docked panel.
    fn settings_body(&mut self, ui: &mut egui::Ui) {
        let m = egui::Margin::symmetric(6.0, 5.0);
        ui.set_width(320.0);
        ui.add_space(2.0);
        // Always expanded. Collapsing these behind a "Change" button read as a
        // dead end rather than a tidy summary.
        ui.label(egui::RichText::new("Server").size(11.0).color(T_MUTED));
        ui.add(egui::TextEdit::singleline(&mut self.server).desired_width(320.0).margin(m));
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Username").size(11.0).color(T_MUTED));
        ui.add(egui::TextEdit::singleline(&mut self.user).desired_width(320.0).margin(m));
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Password").size(11.0).color(T_MUTED));
        ui.add(
            egui::TextEdit::singleline(&mut self.pass)
                .desired_width(320.0)
                .password(true)
                .margin(m),
        );
        ui.add_space(10.0);
        let connecting = self.rx.is_some();
        let label = if connecting {
            "Connecting..."
        } else if self.client.is_some() {
            "Reconnect"
        } else {
            "Connect"
        };
        if ui.add_sized([320.0, 30.0], egui::Button::new(label)).clicked() && !connecting {
            save_config(&self.config_snapshot());
            self.spawn_login();
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Live format").size(11.0).color(T_MUTED));
            egui::ComboBox::from_id_source("fmt")
                .width(80.0)
                .selected_text(self.live_format.clone())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.live_format, "ts".into(), "ts");
                    ui.selectable_value(&mut self.live_format, "m3u8".into(), "m3u8");
                });
        });
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Picture")
                .size(10.5)
                .color(T_MUTED),
        );
        ui.add_space(4.0);
        ui.spacing_mut().slider_width = 170.0;
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.add_sized(
                [72.0, 18.0],
                egui::Label::new(egui::RichText::new("Brightness").size(11.0)),
            );
            changed |= ui.add(egui::Slider::new(&mut self.bright, 0.5..=2.0)).changed();
        });
        ui.horizontal(|ui| {
            ui.add_sized(
                [72.0, 18.0],
                egui::Label::new(egui::RichText::new("Contrast").size(11.0)),
            );
            changed |= ui.add(egui::Slider::new(&mut self.contrast, 0.5..=2.0)).changed();
        });
        ui.horizontal(|ui| {
            ui.add_sized(
                [72.0, 18.0],
                egui::Label::new(egui::RichText::new("Gamma").size(11.0)),
            );
            changed |= ui.add(egui::Slider::new(&mut self.gamma, 0.5..=3.0)).changed();
        });
        if ui.button("Reset picture").clicked() {
            self.bright = 1.0;
            self.contrast = 1.0;
            self.gamma = 1.0;
            changed = true;
        }
        if changed {
            self.player.set_picture(self.bright, self.contrast, self.gamma);
            // Remember it against the stream that is on, so coming back to this
            // channel re-applies it. The write itself is debounced in
            // persist_picture, and the status line is set there so it reports a
            // save that has actually happened.
            if self.playing_id.is_empty() {
                self.status = "Picture changes are only kept for a stream that is playing".into();
            } else {
                self.picture_dirty = true;
            }
        }
        if !self.playing_id.is_empty() {
            let stored = self.pictures.iter().find(|p| p.id == self.playing_id);
            let hint = match stored {
                Some(_) => "Saved for this channel; it comes back next time you open it.",
                None => "Adjusting here remembers the setting for this channel.",
            };
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(egui::RichText::new(hint).size(10.5).color(T_MUTED)).wrap(),
            );
        }
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Presence — who else is on this line")
                .size(10.5)
                .color(T_MUTED),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Watching as").size(11.0).color(T_MUTED));
            self.profile_combo(ui, "profile_settings", 150.0);
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Presence URL").size(11.0).color(T_MUTED));
        ui.add(
            egui::TextEdit::singleline(&mut self.presence_url)
                .desired_width(320.0)
                .margin(m)
                .hint_text("https://eglass-presence.<sub>.workers.dev"),
        );
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Presence token").size(11.0).color(T_MUTED));
        ui.add(
            egui::TextEdit::singleline(&mut self.presence_token)
                .desired_width(320.0)
                .password(true)
                .margin(m),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let url = self.dashboard_url();
            ui.add_enabled_ui(!url.is_empty(), |ui| {
                if ui.button("Open dashboard").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url.clone()));
                }
            });
            if ui.button("Refresh").clicked() {
                if let Ok(mut g) = self.shared.lock() {
                    g.force_poll = true;
                }
            }
        });
        if !self.presence_ready() {
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "Pick a name and fill in both presence fields to see the others.",
                    )
                    .size(10.5)
                    .color(T_MUTED),
                )
                .wrap(),
            );
        }
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);
        // Diagnostics for the one failure mode that is invisible otherwise:
        // clicking a listing and nothing happening.
        let (colour, text) = if launch::scheme_is_registered() {
            (T_SUCCESS, "TV guide links are registered (eagleglass://)")
        } else {
            (T_DANGER, "TV guide links are NOT registered")
        };
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, colour);
            ui.add(egui::Label::new(egui::RichText::new(text).size(10.5).color(T_MUTED)));
        });
        ui.add_space(6.0);
        if self.confirm_reset {
            ui.label(
                egui::RichText::new(
                    "Delete the local settings file and go back to setup?",
                )
                .size(11.0)
                .color(T_DANGER),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add_sized([150.0, 26.0], egui::Button::new("Cancel")).clicked() {
                    self.confirm_reset = false;
                }
                if ui
                    .add_sized([150.0, 26.0], egui::Button::new("Delete and reset"))
                    .clicked()
                {
                    self.confirm_reset = false;
                    self.reset_app();
                }
            });
        } else if ui
            .add_sized([320.0, 26.0], egui::Button::new("Reset app…"))
            .clicked()
        {
            self.confirm_reset = true;
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Ok(handle) = frame.window_handle() {
            if let raw_window_handle::RawWindowHandle::Win32(w) = handle.as_raw() {
                self.player.set_parent(w.hwnd.get() as *mut std::ffi::c_void);
            }
        }
        if self.autostart {
            self.autostart = false;
            if !self.user.is_empty() && !self.pass.is_empty() {
                self.spawn_login();
            }
        }
        self.pump();

        let f11 = ctx.input(|i| i.key_pressed(egui::Key::F11));
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if f11 {
            let on = !self.fullscreen;
            self.set_fullscreen(ctx, on);
        }
        if self.fullscreen && esc {
            self.set_fullscreen(ctx, false);
        }
        let lbtn = video::left_button_down();
        if lbtn && !self.prev_lbtn && self.player.cursor_over_video() {
            let t = ctx.input(|i| i.time);
            if t - self.last_click < 0.45 {
                let on = !self.fullscreen;
                self.set_fullscreen(ctx, on);
                self.last_click = 0.0;
            } else {
                self.last_click = t;
            }
        }
        self.prev_lbtn = lbtn;

        self.ensure_presence_thread();
        self.presence_sync();
        self.persist_volume(ctx);
        self.persist_picture(ctx);
        self.poll_launch_request(ctx);
        self.follow_launch_in_sidebar();
        self.detect_drop();
        self.check_stream_drop();

        if self.player.is_playing() {
            ctx.request_repaint_after(std::time::Duration::from_millis(25));
        } else {
            // Even idle, keep one tick a second so a request from the guide page
            // is noticed promptly. Idle egui repaints on demand only, so without
            // this a click in the browser could sit unseen indefinitely.
            ctx.request_repaint_after(std::time::Duration::from_millis(1000));
        }

        if !ctx.wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
                self.toggle_pause();
            }
            if ctx.input(|i| i.key_pressed(egui::Key::M)) {
                self.toggle_mute();
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                let v = (self.player.volume() + 5).min(100);
                self.player.set_volume(v);
                self.muted = false;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                let v = (self.player.volume() - 5).max(0);
                self.player.set_volume(v);
            }
        }
        if self.fullscreen {
            let mut vr: Option<egui::Rect> = None;
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(egui::Color32::BLACK))
                .show(ctx, |ui| {
                    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click());
                    if resp.double_clicked() {
                        self.set_fullscreen(ctx, false);
                    }
                    vr = Some(rect);
                });
            if let Some(rect) = vr {
                self.apply_bounds(ctx, rect);
            }
            return;
        }

        let initial_vol = self.player.volume();
        let mut vol_val = initial_vol;
        let mut act_pause = false;
        let mut act_mute = false;
        let mut want_fs = false;
        let playing = self.player.is_playing();

        if self.wizard && self.client.is_none() {
            self.wizard_ui(ctx);
            return;
        }

        if self.side_width.is_none() && !self.cats.is_empty() {
            let font = egui::FontId::proportional(14.0);
            let mut maxw: f32 = 120.0;
            for c in &self.cats {
                let g = ctx.fonts(|f| f.layout_no_wrap(c.name.clone(), font.clone(), egui::Color32::WHITE));
                maxw = maxw.max(g.size().x);
            }
            for kind in [Kind::Live, Kind::Vod, Kind::Series] {
                let n = self.cats.iter().filter(|c| c.kind == kind).count();
                let label = format!("{} ({})", kind.label(), n);
                let g = ctx.fonts(|f| f.layout_no_wrap(label, font.clone(), egui::Color32::WHITE));
                maxw = maxw.max(g.size().x);
            }
            self.side_width = Some((maxw + 26.0 + 44.0).clamp(240.0, 420.0));
        }

        egui::TopBottomPanel::top("top")
            .frame(egui::Frame::side_top_panel(&ctx.style()).fill(T_TOP))
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let connecting = self.rx.is_some();
                    let (col, text) = if self.client.is_some() {
                        (T_SUCCESS, format!("Connected · {}", self.user))
                    } else if connecting {
                        (T_ACCENT, "Connecting…".to_string())
                    } else {
                        (T_DANGER, "Not connected".to_string())
                    };
                    pill(ui, col, &text);
                    ui.add_space(4.0);
                    self.presence_top_ui(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_sized([34.0, 26.0], egui::Button::new("⚙")).clicked() {
                            self.show_settings = !self.show_settings;
                            if !self.show_settings {
                                self.confirm_reset = false;
                            }
                        }
                    });
                });
                ui.add_space(6.0);
            });

        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::side_top_panel(&ctx.style()).fill(T_TOP))
            .show(ctx, |ui| {
                ui.label(self.status.clone());
            });

        // Settings as a docked panel rather than a floating window or an overlay.
        // It is declared before the video's central panel, so the video simply
        // gets a narrower rectangle and keeps playing - nothing has to be hidden
        // to make room for it.
        if self.show_settings {
            egui::SidePanel::right("settings")
                .resizable(false)
                .exact_width(344.0)
                .frame(egui::Frame::side_top_panel(&ctx.style()).fill(T_SURFACE))
                .show(ctx, |ui| {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Settings").size(15.0).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add_sized([28.0, 22.0], egui::Button::new("✕")).clicked() {
                                self.show_settings = false;
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.separator();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        self.settings_body(ui);
                    });
                });
        }

        let mut cat_action: Option<(Kind, Option<String>)> = None;
        let mut switch_kind: Option<Kind> = None;
        let mut play_side: Option<(Kind, usize, Item)> = None;
        let side_w = self.side_width.unwrap_or(280.0);
        let apply_exact = !self.width_applied && self.side_width.is_some();
        let mut open_cats = std::mem::take(&mut self.open_cats);
        let scroll_to = std::mem::take(&mut self.scroll_to_selected);
        {
            let cats = &self.cats;
            let items = &self.items;
            let current = self.cat.clone();
            let cur_kind = self.kind;
            let selected = self.selected;
            let frame = egui::Frame::side_top_panel(&ctx.style()).fill(T_SIDEBAR);
            let panel = if apply_exact {
                egui::SidePanel::left("cats").exact_width(side_w).frame(frame.clone())
            } else {
                egui::SidePanel::left("cats")
                    .resizable(true)
                    .default_width(side_w)
                    .width_range(200.0..=440.0)
                    .frame(frame.clone())
            };
            let mut open = [self.open_live, self.open_vod, self.open_series];
            panel.show(ctx, |ui| {
                ui.add_space(6.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.add_space(6.0);
                    for (i, kind) in [Kind::Live, Kind::Vod, Kind::Series].iter().enumerate() {
                        let rows: Vec<&CatRow> = cats.iter().filter(|c| c.kind == *kind).collect();
                        let total = if *kind == cur_kind { items.len() } else { 0 };
                        if side_header(ui, open[i], kind.label(), total) {
                            open[i] = !open[i];
                            if open[i] && *kind != cur_kind {
                                switch_kind = Some(*kind);
                            }
                        }
                        if open[i] {
                            if side_row(ui, current.is_none() && *kind == cur_kind, "All channels") {
                                cat_action = Some((*kind, None));
                                if *kind != cur_kind {
                                    switch_kind = Some(*kind);
                                }
                            }
                            for r in rows {
                                let expanded = open_cats.contains(&r.id);
                                let count = if *kind == cur_kind {
                                    items.iter().filter(|it| it.cat == r.id).count()
                                } else {
                                    0
                                };
                                let cat_sel = current.as_deref() == Some(r.id.as_str()) && *kind == cur_kind;
                                if cat_node(ui, expanded, cat_sel, &r.name, count) {
                                    if expanded {
                                        open_cats.remove(&r.id);
                                    } else {
                                        open_cats.insert(r.id.clone());
                                        if *kind != cur_kind {
                                            switch_kind = Some(*kind);
                                        }
                                        cat_action = Some((*kind, Some(r.id.clone())));
                                    }
                                }
                                if expanded && *kind == cur_kind {
                                    for (idx, it) in items.iter().enumerate() {
                                        if it.cat != r.id {
                                            continue;
                                        }
                                        let row = side_chan(ui, selected == Some(idx), &it.name);
                                        if selected == Some(idx) && scroll_to {
                                            row.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        if row.clicked() {
                                            play_side = Some((*kind, idx, it.clone()));
                                        }
                                    }
                                }
                            }
                        }
                        ui.add_space(6.0);
                    }
                });
            });
            self.open_live = open[0];
            self.open_vod = open[1];
            self.open_series = open[2];
        }
        self.open_cats = open_cats;
        if apply_exact {
            self.width_applied = true;
        }
        if let Some(kind) = switch_kind {
            if kind != self.kind {
                self.kind = kind;
                self.spawn_items(kind, None);
            }
        }
        if let Some((kind, cat)) = cat_action {
            self.kind = kind;
            self.cat = cat.clone();
        }
        if let Some((kind, idx, item)) = play_side {
            self.selected = Some(idx);
            self.spawn_play(kind, item);
        }

        let selected = self.selected;
        let mut video_rect: Option<egui::Rect> = None;

        let title = self.now_playing.clone();
        let alpha = 1.0_f32;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if !title.is_empty() {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&title).size(13.0).color(T_TEXT))
                            .truncate(),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if playing {
                        live_badge(ui, 1.0);
                    }
                });
            });
            ui.add_space(4.0);
            let avail = ui.available_size();
            let mut vh = avail.x * 9.0 / 16.0;
            let max_vh = (avail.y - 46.0).max(120.0);
            if vh > max_vh {
                vh = max_vh;
            }
            let vw = (vh * 16.0 / 9.0).min(avail.x);
            let (vrect, vresp) = ui.allocate_exact_size(
                egui::vec2(avail.x, vh.max(80.0)),
                egui::Sense::click(),
            );
            ui.painter().rect_filled(vrect, 0.0, egui::Color32::BLACK);
            let video = egui::Rect::from_center_size(vrect.center(), egui::vec2(vw, vh));
            video_rect = Some(video);
            if vresp.double_clicked() {
                want_fs = true;
            }
            if vresp.hovered() {
                let scroll = ui.input(|i| i.raw_scroll_delta.y);
                if scroll.abs() > 0.0 {
                    vol_val = (vol_val + (scroll / 40.0) as i32).clamp(0, 100);
                }
            }

            let (bar, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 40.0),
                egui::Sense::hover(),
            );
            ui.add_space(6.0);
            let cy = bar.center().y;
            let col = egui::Color32::from_white_alpha((alpha * 235.0) as u8);
            let hov = egui::Color32::from_white_alpha((alpha * 26.0) as u8);

            let btn = egui::Rect::from_center_size(egui::pos2(bar.min.x + 21.0, cy), egui::vec2(30.0, 30.0));
            let r = ui.interact(btn, ui.id().with("pp"), egui::Sense::click());
            if r.hovered() {
                ui.painter().rect_filled(btn, 5.0, hov);
            }
            if playing {
                paint_pause(ui.painter(), btn.shrink(9.0), col);
            } else {
                paint_play(ui.painter(), btn.shrink(9.0), col);
            }
            if r.clicked() {
                act_pause = true;
            }

            let vbtn = egui::Rect::from_center_size(egui::pos2(bar.min.x + 58.0, cy), egui::vec2(26.0, 26.0));
            let vr = ui.interact(vbtn, ui.id().with("volicon"), egui::Sense::click());
            if vr.hovered() {
                ui.painter().rect_filled(vbtn, 5.0, hov);
            }
            let vcol = if vol_val == 0 {
                egui::Color32::from_white_alpha((alpha * 120.0) as u8)
            } else {
                col
            };
            paint_speaker(ui.painter(), vbtn.shrink(6.0), vcol);
            if vr.clicked() {
                act_mute = true;
            }

            let vs = egui::Rect::from_min_size(egui::pos2(bar.min.x + 76.0, cy - 9.0), egui::vec2(120.0, 18.0));
            let vsr = ui.interact(vs, ui.id().with("vol"), egui::Sense::click_and_drag());
            let track = egui::Rect::from_center_size(vs.center(), egui::vec2(vs.width(), 4.0));
            ui.painter().rect_filled(track, 2.0, egui::Color32::from_white_alpha((alpha * 70.0) as u8));
            let mut nv = vol_val;
            if let Some(p) = vsr.interact_pointer_pos() {
                nv = (((p.x - track.min.x) / track.width()).clamp(0.0, 1.0) * 100.0).round() as i32;
            }
            let fw = track.width() * (nv as f32 / 100.0);
            ui.painter().rect_filled(
                egui::Rect::from_min_size(track.min, egui::vec2(fw, track.height())),
                2.0,
                egui::Color32::from_white_alpha((alpha * 235.0) as u8),
            );
            ui.painter()
                .circle_filled(egui::pos2(track.min.x + fw, track.center().y), 5.0, col);
            if nv != vol_val {
                vol_val = nv;
            }

            let fs = egui::Rect::from_center_size(egui::pos2(bar.max.x - 21.0, cy), egui::vec2(30.0, 30.0));
            let fr = ui.interact(fs, ui.id().with("fs"), egui::Sense::click());
            if fr.hovered() {
                ui.painter().rect_filled(fs, 5.0, hov);
            }
            paint_fullscreen(ui.painter(), fs.shrink(9.0), col);
            if fr.clicked() {
                want_fs = true;
            }
            let _ = video;
        });

        if act_pause {
            self.toggle_pause();
        }
        if act_mute {
            self.toggle_mute();
        }
        if vol_val != initial_vol {
            self.player.set_volume(vol_val);
            if vol_val > 0 {
                self.muted = false;
            }
        }
        if self.vol_reapply > 0 && playing {
            self.player.set_volume(self.player.volume());
            self.player.set_picture(self.bright, self.contrast, self.gamma);
            self.vol_reapply -= 1;
            ctx.request_repaint();
        }
        if want_fs {
            self.set_fullscreen(ctx, true);
        }
        // The guard dialog sits over the video surface, which egui cannot paint
        // on top of, so shrink the video away while it is up. Nothing is playing
        // at that point, so there is nothing to interrupt.
        //
        // Settings deliberately does NOT do this any more: it is a docked panel,
        // so the video rectangle just gets narrower and keeps playing.
        if self.pending_play.is_some() {
            self.player.set_bounds(0.0, 0.0, 1.0, 1.0);
        } else if let Some(rect) = video_rect {
            self.apply_bounds(ctx, rect);
        }


        self.guard_dialog(ctx);

        self.selected = selected;
    }

    /// Clearing on the way out means the others see you leave immediately
    /// instead of waiting out the TTL. If the process is killed this never
    /// runs, which is exactly why the worker also expires presence.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        presence::clear_on_exit(&self.shared);
    }
}

fn parse_flags(args: &[String]) -> (String, String, String, Vec<String>) {
    let mut server = std::env::var("EG_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.to_string());
    let mut user = std::env::var("EG_USER").unwrap_or_default();
    let mut pass = std::env::var("EG_PASS").unwrap_or_default();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--server" if i + 1 < args.len() => { server = args[i + 1].clone(); i += 2; }
            "--user" if i + 1 < args.len() => { user = args[i + 1].clone(); i += 2; }
            "--pass" if i + 1 < args.len() => { pass = args[i + 1].clone(); i += 2; }
            other => { rest.push(other.to_string()); i += 1; }
        }
    }
    (server, user, pass, rest)
}

fn kind_from(s: &str) -> Option<Kind> {
    match s {
        "live" => Some(Kind::Live),
        "vod" | "movie" | "movies" => Some(Kind::Vod),
        "series" => Some(Kind::Series),
        _ => None,
    }
}

fn run_cli(server: String, user: String, pass: String, rest: Vec<String>) -> Result<(), String> {
    let cmd = rest.first().map(|s| s.as_str()).unwrap_or("categories");

    // The presence roster is about the shared worker, not the panel, so it must
    // not require panel credentials to be present.
    if cmd == "presence" {
        return presence_cli(rest.get(1).map(|s| s.as_str()));
    }
    // Setting a name is likewise about the worker, not the panel.
    if cmd == "profile" {
        return set_profile(rest.get(1).map(|s| s.as_str()));
    }

    let client = Xtream::new(&server, &user, &pass)?;
    match cmd {
        "categories" => {
            client.authenticate()?;
            for (kind, cats) in [
                (Kind::Live, client.live_categories()?),
                (Kind::Vod, client.vod_categories()?),
                (Kind::Series, client.series_categories()?),
            ] {
                println!("== {} ({}) ==", kind.label(), cats.len());
                for c in cats {
                    println!("  {:>7}  {}", c.id, c.name);
                }
            }
        }
        "list" => {
            let kind = kind_from(rest.get(1).map(|s| s.as_str()).unwrap_or("live"))
                .ok_or("usage: list <live|vod|series> [category_id]")?;
            let cat = rest.get(2).map(|s| s.as_str());
            client.authenticate()?;
            for it in client.category_items(kind, cat)? {
                println!("  {:>10}  {}", it.id, it.name);
            }
        }
        "url" | "play" => {
            let kind = kind_from(rest.get(1).map(|s| s.as_str()).unwrap_or("live"))
                .ok_or("usage: url <live|vod|series> <id> [format]")?;
            let id = rest.get(2).ok_or("missing id")?;
            let fmt = rest.get(3).map(|s| s.as_str()).unwrap_or("ts");
            let item = Item { id: id.clone(), name: String::new(), ext: String::new(), cat: String::new() };
            let url = client.play_url(kind, &item, fmt)?;
            println!("{}", url);
            if cmd == "play" {
                play_url(&url)?;
            }
        }
        "m3u" => {
            let out = rest.get(1).cloned().unwrap_or_else(|| "playlist.m3u".to_string());
            let text = client.m3u_playlist()?;
            std::fs::write(&out, text).map_err(|e| e.to_string())?;
            println!("wrote {}", out);
        }
        "epg" => {
            println!("{}", client.xmltv_url());
        }
        other => return Err(format!("unknown command: {}", other)),
    }
    Ok(())
}

/// The names the worker knows about, falling back to the built-in four when the
/// worker cannot be reached.
fn known_profiles(cfg: &Config) -> Vec<String> {
    if !cfg.presence_url.is_empty() && !cfg.presence_token.is_empty() {
        if let Ok(list) =
            presence::Presence::new(&cfg.presence_url, &cfg.presence_token).profiles()
        {
            if !list.is_empty() {
                return list.into_iter().map(|(_, name)| name).collect();
            }
        }
    }
    presence::DEFAULT_PROFILES
        .iter()
        .map(|(_, name)| name.to_string())
        .collect()
}

/// `eglass.exe profile Blain`
///
/// Setting up a friend's machine should not depend on them finding the right
/// dropdown, so the name can be set from a script - and it is the only setting
/// that differs between friends, since the panel and presence credentials are
/// shared.
fn set_profile(name: Option<&str>) -> Result<(), String> {
    let mut cfg = load_config();
    let known = known_profiles(&cfg);

    let name = match name {
        Some(n) => n,
        None => {
            return Err(format!(
                "usage: eglass.exe profile <name>\n  known: {}\n  currently: {}",
                known.join(", "),
                if cfg.profile.is_empty() { "(not set)" } else { &cfg.profile }
            ))
        }
    };

    let canonical = known
        .iter()
        .find(|n| n.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("unknown name '{}'. known: {}", name, known.join(", ")))?;

    cfg.profile = canonical.clone();
    if cfg.device_id.is_empty() {
        cfg.device_id = presence::new_device_id();
    }
    save_config(&cfg);
    println!(
        "{} now watches as {}",
        presence::device_name(),
        canonical
    );
    Ok(())
}

/// `eglass.exe presence [watch|stop|guard]` - check the shared roster without the GUI,
/// and the quickest way to prove the worker is reachable.
fn presence_cli(action: Option<&str>) -> Result<(), String> {
    let mut cfg = load_config();
    if cfg.device_id.is_empty() {
        // The GUI generates this on first run; CLI use should not be a
        // different device every time.
        cfg.device_id = presence::new_device_id();
        save_config(&cfg);
    }
    let url = std::env::var("EG_PRESENCE_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.presence_url.clone());
    let token = std::env::var("EG_PRESENCE_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.presence_token.clone());
    let profile = std::env::var("EG_PROFILE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.profile.clone());

    if url.is_empty() || token.is_empty() {
        return Err(
            "presence_url and presence_token are not set. Add them to %APPDATA%\\EagleGlass\\config.txt"
                .into(),
        );
    }
    if profile.is_empty() {
        return Err("profile is not set. Add e.g. `profile=Peter` to config.txt".into());
    }

    let pid = profile.to_lowercase();
    // The panel connection follows the same override rules as the GUI, so
    // `presence guard` can be pointed at a test panel without editing config.
    let server = std::env::var("EG_SERVER")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.server.clone());
    let user = std::env::var("EG_USER")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.user.clone());
    let pass = std::env::var("EG_PASS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.pass.clone());
    let client = presence::Presence::new(&url, &token);

    match action {
        Some("watch") => {
            let shared = Shared {
                enabled: true,
                url,
                token,
                profile: pid.clone(),
                device_id: cfg.device_id.clone(),
                device: presence::device_name(),
                active: true,
                state: "playing".to_string(),
                stream_kind: "live".to_string(),
                stream_id: "cli".to_string(),
                stream_name: "CLI test stream".to_string(),
                since: presence::now_ms(),
                ..Default::default()
            };
            let r = client.heartbeat(&shared)?;
            println!("{} checked in as \"{}\"\n", pid, presence::device_name());
            print_roster(&r, &pid);
        }
        Some("stop") => {
            client.idle(&pid, &cfg.device_id)?;
            println!("{} cleared from the roster.", pid);
        }
        Some("guard") => {
            // Answers "would I be warned before pressing play right now?" -
            // the one decision that is otherwise only reachable through the GUI.
            println!("panel   {}", server);
            let counts = match probe_panel(&server, &user, &pass) {
                Ok(v) => {
                    println!("        active_cons={} max_connections={}", v.0, v.1);
                    Some(v)
                }
                Err(e) => {
                    println!("        unreachable: {}", e);
                    None
                }
            };
            let roster = client.roster().ok();
            match &roster {
                Some(r) => println!(
                    "roster  {} device(s), {} other(s), limit {}",
                    r.watchers.len(),
                    r.others(&pid).len(),
                    r.max_connections
                ),
                None => println!("roster  unreachable"),
            }
            let Some((active, maxc)) = counts else {
                println!("guard   cannot judge - the panel could not be asked");
                return Ok(());
            };
            match decide_guard(active, maxc, roster.as_ref(), &pid) {
                GuardOutcome::Free => println!("guard   no warning - the stream would just start"),
                GuardOutcome::Proceed { note } => {
                    println!("guard   starts immediately, with a status note");
                    println!("        \"{}\"", note);
                }
                GuardOutcome::Warn(c) => {
                    println!("guard   WARN before play");
                    println!("        \"{} is watching\"", c.who);
                    if !c.stream.is_empty() {
                        println!("        \"{} · {}m in\"", c.stream, c.minutes);
                    }
                    println!(
                        "        \"This line allows {} {}. Starting will kick them off.\"",
                        c.max_connections.max(1),
                        if c.max_connections.max(1) == 1 { "connection" } else { "connections" }
                    );
                }
            }
        }
        _ => {
            let r = client.roster()?;
            print_roster(&r, &pid);
        }
    }
    Ok(())
}

fn print_roster(r: &Roster, me: &str) {
    if let Some(err) = &r.error {
        println!("error: {}", err);
        return;
    }
    let account = if r.max_connections > 0 {
        format!("connections {}/{}", r.active_cons, r.max_connections)
    } else {
        "connection limit not reported".to_string()
    };
    println!(
        "{} · {} watching",
        account,
        r.watchers.len()
    );
    if r.unaccounted > 0 {
        println!(
            "  ! {} connection(s) with no matching client - someone outside the group may be watching",
            r.unaccounted
        );
    }
    if r.watchers.is_empty() {
        println!("  nobody is watching");
        return;
    }
    for w in &r.watchers {
        let who = if w.profile == me {
            format!("{} (you)", w.name)
        } else {
            w.name.clone()
        };
        let chan = clip(&w.stream_name, 30);
        println!(
            "  {:<16} {:<30} {:>7}  {}{}",
            who,
            chan,
            presence::human_duration(w.watching_seconds),
            w.device,
            if w.state == "paused" { "  [paused]" } else { "" }
        );
    }
}

/// The pill's colour and text.
///
/// Pure, and tested, because the first version was wrong here: it answered
/// "how many connections are in use", which is the wrong question on a
/// one-connection line - it reads as an alarm whenever you are watching your
/// own channel, and it hides *who* has the line at the moment that matters most.
fn pill_state(roster: &Roster, me: &str) -> (egui::Color32, String) {
    let mut names: Vec<String> = roster
        .others(me)
        .into_iter()
        .map(|w| w.name.clone())
        .collect();
    names.sort();
    names.dedup();
    let mine = roster.mine(me);
    let maxc = roster.max_connections;
    let one_slot = maxc == 1;

    if roster.unaccounted > 0 {
        // The panel counts a connection that no client of ours claims.
        return (T_DANGER, "Line in use · unknown".to_string());
    }
    if one_slot && mine && !names.is_empty() {
        return (T_DANGER, format!("Conflict · {} also on", clip(&names.join(", "), 22)));
    }
    if !names.is_empty() {
        return if one_slot {
            let first = roster.others(me)[0];
            let what = if first.stream_name.is_empty() {
                String::new()
            } else {
                format!(" · {}", clip(&first.stream_name, 26))
            };
            (T_ACCENT, format!("{}{}", clip(&names.join(", "), 20), what))
        } else {
            let label = if names.len() > 2 {
                format!("{} +{}", names[..2].join(", "), names.len() - 2)
            } else {
                names.join(", ")
            };
            (T_ACCENT, format!("{} watching · {}", names.len(), label))
        };
    }
    if mine {
        return (
            T_SUCCESS,
            if maxc > 1 {
                format!("Line is yours · {}/{}", roster.active_cons, maxc)
            } else {
                "Line is yours".to_string()
            },
        );
    }
    (
        T_SUCCESS,
        if maxc > 1 {
            format!("Line free · {}/{}", roster.active_cons, maxc)
        } else {
            "Line free".to_string()
        },
    )
}

/// How to refer to whoever holds the line, in a sentence.
fn describe_conflict(c: &Conflict) -> String {
    if c.is_unknown() {
        "someone".to_string()
    } else {
        c.who.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watcher(profile: &str, name: &str, stream: &str, secs: i64) -> presence::Watcher {
        presence::Watcher {
            profile: profile.to_string(),
            name: name.to_string(),
            device: format!("{}-PC", name),
            state: "playing".to_string(),
            stream_name: stream.to_string(),
            age_seconds: 3,
            watching_seconds: secs,
        }
    }

    /// Build a roster the way the worker would: freshest first, and with
    /// `unaccounted` already derived from active_cons against device count.
    fn roster(active_cons: i64, max_connections: i64, watchers: Vec<presence::Watcher>) -> Roster {
        let unaccounted = if max_connections > 0 {
            (active_cons - watchers.len() as i64).max(0)
        } else {
            0
        };
        Roster {
            watchers,
            active_cons,
            max_connections,
            unaccounted,
            fetched_at: presence::now_ms(),
            profiles: Vec::new(),
            error: None,
        }
    }

    const F1: &str = "Sky Sports F1";

    /* ------------------------------------------------ pill: one connection */

    #[test]
    fn one_connection_free_reads_as_free() {
        let (color, text) = pill_state(&roster(0, 1, vec![]), "peter");
        assert_eq!(text, "Line free");
        assert_eq!(color, T_SUCCESS, "a free line must not look like an alarm");
    }

    // The bug this whole change exists for: watching your own channel on a
    // one-connection line used to render as a red "1/1 connections in use".
    #[test]
    fn one_connection_mine_is_not_an_alarm() {
        let r = roster(1, 1, vec![watcher("peter", "Peter", F1, 600)]);
        let (color, text) = pill_state(&r, "peter");
        assert_eq!(text, "Line is yours");
        assert_eq!(color, T_SUCCESS, "your own stream is not a conflict");
        assert_eq!(r.unaccounted, 0, "our own connection is accounted for");
    }

    #[test]
    fn one_connection_taken_names_who_and_what() {
        let r = roster(1, 1, vec![watcher("blain", "Blain", F1, 2280)]);
        let (color, text) = pill_state(&r, "peter");
        assert_eq!(text, "Blain · Sky Sports F1");
        assert_eq!(color, T_ACCENT);
    }

    #[test]
    fn one_connection_two_of_us_is_a_conflict() {
        let r = roster(
            1,
            1,
            vec![
                watcher("blain", "Blain", F1, 60),
                watcher("peter", "Peter", "Sky Sports Main Event", 30),
            ],
        );
        let (color, text) = pill_state(&r, "peter");
        assert_eq!(text, "Conflict · Blain also on");
        assert_eq!(color, T_DANGER);
        assert_eq!(r.unaccounted, 0, "two devices, one connection: nothing unaccounted");
    }

    #[test]
    fn one_connection_used_by_a_stranger_is_flagged() {
        let r = roster(1, 1, vec![]);
        assert_eq!(r.unaccounted, 1);
        let (color, text) = pill_state(&r, "peter");
        assert_eq!(text, "Line in use · unknown");
        assert_eq!(color, T_DANGER);
    }

    #[test]
    fn a_slot_limit_we_do_not_know_about_is_not_treated_as_one() {
        // maxConnections 0 means "nobody has reported it yet", which must not
        // be silently treated as a single-connection line.
        let r = roster(1, 0, vec![watcher("blain", "Blain", F1, 60)]);
        let (_, text) = pill_state(&r, "peter");
        assert_eq!(text, "1 watching · Blain");
        assert_eq!(r.unaccounted, 0, "cannot be unaccounted without a known limit");
    }

    /* ------------------------------------------------ pill: multi connection */

    #[test]
    fn multi_connection_mostly_free_reports_ours() {
        let r = roster(1, 4, vec![watcher("peter", "Peter", F1, 60)]);
        assert_eq!(r.unaccounted, 0);
        let (color, text) = pill_state(&r, "peter");
        assert_eq!(text, "Line is yours · 1/4");
        assert_eq!(color, T_SUCCESS);
    }

    // A *full* line is not an alarm when every connection on it is one of us.
    #[test]
    fn multi_connection_full_is_not_an_alarm() {
        let r = roster(
            4,
            4,
            vec![
                watcher("peter", "Peter", F1, 60),
                watcher("blain", "Blain", F1, 55),
                watcher("johnny", "Johnny", F1, 50),
                watcher("panda", "Panda", F1, 45),
            ],
        );
        assert_eq!(r.unaccounted, 0, "four people, four connections: all accounted for");
        let (color, text) = pill_state(&r, "peter");
        assert_eq!(text, "3 watching · Blain, Johnny +1");
        assert_ne!(color, T_DANGER, "a full line with only friends on it is not a conflict");
    }

    #[test]
    fn multi_connection_counts_others() {
        let r = roster(
            2,
            4,
            vec![watcher("peter", "Peter", F1, 60), watcher("blain", "Blain", F1, 30)],
        );
        let (color, text) = pill_state(&r, "peter");
        assert_eq!(text, "1 watching · Blain");
        assert_eq!(color, T_ACCENT);
    }

    #[test]
    fn multi_connection_ignores_my_own_entry() {
        let r = roster(1, 4, vec![watcher("peter", "Peter", F1, 60)]);
        let (_, text) = pill_state(&r, "peter");
        assert_eq!(text, "Line is yours · 1/4", "our own entry is not an 'other'");
    }

    /* ---------------------------------------------------- per-channel picture */

    fn pic(id: &str, b: f32, c: f32, g: f32) -> Picture {
        Picture {
            id: id.to_string(),
            name: format!("channel {}", id),
            bright: b,
            contrast: c,
            gamma: g,
        }
    }

    #[test]
    fn an_untuned_channel_gets_untouched_defaults() {
        let pics = vec![pic("228637", 1.5, 1.2, 0.9)];
        assert_eq!(picture_for(&pics, "228637"), (1.5, 1.2, 0.9));
        assert_eq!(picture_for(&pics, "91203"), (1.0, 1.0, 1.0));
        assert_eq!(picture_for(&[], "anything"), (1.0, 1.0, 1.0));
    }

    #[test]
    fn tuning_a_channel_records_it() {
        let mut pics = Vec::new();
        upsert_picture(&mut pics, "228637", "Sky Sports Main Event", 1.4, 1.1, 0.8);
        assert_eq!(picture_for(&pics, "228637"), (1.4, 1.1, 0.8));
        assert_eq!(pics.len(), 1);
    }

    #[test]
    fn retuning_replaces_rather_than_appends() {
        let mut pics = Vec::new();
        upsert_picture(&mut pics, "228637", "one", 1.4, 1.0, 1.0);
        upsert_picture(&mut pics, "228637", "one", 1.9, 1.0, 1.0);
        assert_eq!(pics.len(), 1, "same channel must not accumulate entries");
        assert_eq!(picture_for(&pics, "228637"), (1.9, 1.0, 1.0));
    }

    #[test]
    fn retuning_moves_the_entry_to_the_end() {
        let mut pics = Vec::new();
        upsert_picture(&mut pics, "a", "a", 1.1, 1.0, 1.0);
        upsert_picture(&mut pics, "b", "b", 1.2, 1.0, 1.0);
        upsert_picture(&mut pics, "a", "a", 1.3, 1.0, 1.0);
        assert_eq!(pics.last().unwrap().id, "a", "most recent tuning goes last");
    }

    #[test]
    fn a_full_list_drops_the_least_recently_tuned() {
        let mut pics = Vec::new();
        for i in 0..(PIC_LIMIT + 5) {
            upsert_picture(&mut pics, &format!("id{}", i), "n", 1.0, 1.0, 1.0);
        }
        assert_eq!(pics.len(), PIC_LIMIT);
        assert!(picture_for(&pics, "id0") == (1.0, 1.0, 1.0), "oldest dropped");
        assert!(pics.iter().any(|p| p.id == format!("id{}", PIC_LIMIT + 4)), "newest kept");
    }

    #[test]
    fn values_are_rounded_for_a_readable_config() {
        let mut pics = Vec::new();
        upsert_picture(&mut pics, "x", "x", 1.234567, 0.987654, 1.000001);
        let p = &pics[0];
        assert_eq!((p.bright, p.contrast, p.gamma), (1.235, 0.988, 1.0));
    }

    #[test]
    fn a_hand_edited_value_outside_the_slider_range_is_clamped() {
        let mut pics = Vec::new();
        upsert_picture(&mut pics, "x", "x", 99.0, -5.0, 42.0);
        let p = &pics[0];
        assert_eq!(p.bright, 2.0);
        assert_eq!(p.contrast, 0.5);
        assert_eq!(p.gamma, 3.0);
    }

    /* -------------------------------------------------------------- guard */

    fn warned(o: &GuardOutcome) -> Option<Conflict> {
        match o {
            GuardOutcome::Warn(c) => Some(c.clone()),
            _ => None,
        }
    }

    fn note(o: &GuardOutcome) -> Option<String> {
        match o {
            GuardOutcome::Proceed { note } => Some(note.clone()),
            _ => None,
        }
    }

    #[test]
    fn guard_is_silent_on_a_free_line() {
        let r = roster(0, 1, vec![]);
        assert!(matches!(decide_guard(0, 1, Some(&r), "peter"), GuardOutcome::Free));
    }

    #[test]
    fn guard_names_the_other_watcher() {
        let r = roster(1, 1, vec![watcher("blain", "Blain", F1, 38 * 60)]);
        let c = warned(&decide_guard(1, 1, Some(&r), "peter")).expect("should warn");
        assert_eq!(c.who, "Blain");
        assert_eq!(c.stream, F1);
        assert_eq!(c.minutes, 38);
        assert_eq!(c.max_connections, 1);
    }

    #[test]
    fn guard_picks_the_freshest_other_watcher() {
        let r = roster(
            2,
            4,
            vec![watcher("johnny", "Johnny", F1, 60), watcher("blain", "Blain", F1, 30)],
        );
        let c = warned(&decide_guard(2, 4, Some(&r), "peter")).expect("should warn");
        assert_eq!(c.who, "Johnny", "roster is freshest-first");
    }

    #[test]
    fn guard_blames_our_other_device_rather_than_a_stranger() {
        let r = roster(1, 1, vec![watcher("peter", "Peter", F1, 60)]);
        let c = warned(&decide_guard(1, 1, Some(&r), "peter")).expect("should warn");
        assert_eq!(c.who, "You (another device)");
    }

    #[test]
    fn guard_warns_even_when_the_limit_is_unknown() {
        let r = roster(3, 0, vec![watcher("blain", "Blain", F1, 60)]);
        let c = warned(&decide_guard(3, 0, Some(&r), "peter")).expect("busy is busy");
        assert_eq!(c.who, "Blain");
    }

    /// The policy that came out of measuring a real panel: a nonzero
    /// active_cons with nobody checked in must NOT stop the user.
    ///
    /// It can mean a watcher we cannot see, or it can mean the panel's counter
    /// lagging behind a stream that already ended - observed holding at 1 for
    /// over four minutes with nothing of ours running. A modal on that reading
    /// would fire on every single play attempt, so it proceeds with a note.
    #[test]
    fn guard_does_not_block_on_an_unclaimed_connection() {
        let free_roster = roster(1, 1, vec![]);
        let o = decide_guard(1, 1, Some(&free_roster), "peter");
        assert!(warned(&o).is_none(), "an unclaimed connection must not block");
        let n = note(&o).expect("should still say something");
        assert!(n.contains("1/1"), "note should carry the counts: {}", n);
    }

    // Same ambiguity when the worker cannot be reached at all.
    #[test]
    fn guard_still_proceeds_when_the_worker_is_unreachable() {
        let o = decide_guard(1, 1, None, "peter");
        assert!(warned(&o).is_none());
        assert!(note(&o).is_some());
    }

    /// ...but a friend we *can* name still stops it, worker reachable or not.
    #[test]
    fn guard_still_warns_when_it_can_name_someone() {
        let r = roster(1, 1, vec![watcher("blain", "Blain", F1, 60)]);
        assert!(warned(&decide_guard(1, 1, Some(&r), "peter")).is_some());
    }

    /* ------------------------------ a launch from the guide, from cold ---- */

    /// A panel that answers just enough of the Xtream API for a login, a
    /// category list and two category listings. Its thread ends with the
    /// process, which is fine for a test binary.
    fn fake_panel() -> String {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        const LOGIN: &str = r#"{"user_info":{"username":"tester","status":"Active","auth":1,"exp_date":"1893456000","max_connections":"1"}}"#;
        const CATS: &str = r#"[{"category_id":"10","category_name":"UK Sport"},{"category_id":"11","category_name":"News"}]"#;
        const ALL: &str = r#"[{"stream_id":"1","name":"Sky Sports F1","category_id":"10"},{"stream_id":"2","name":"Sky Sports Main Event","category_id":"10"},{"stream_id":"3","name":"BBC News","category_id":"11"}]"#;
        const CAT10: &str = r#"[{"stream_id":"1","name":"Sky Sports F1","category_id":"10"},{"stream_id":"2","name":"Sky Sports Main Event","category_id":"10"}]"#;
        const CAT11: &str = r#"[{"stream_id":"3","name":"BBC News","category_id":"11"}]"#;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a fake panel");
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut sock) = stream else { continue };
                let Ok(dup) = sock.try_clone() else { continue };
                let mut reader = BufReader::new(dup);
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                    continue;
                }
                // Drain the headers; ureq sends no body.
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
                        break;
                    }
                }
                let target = request_line.split_whitespace().nth(1).unwrap_or("/");
                let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
                let arg = |key: &str| -> String {
                    query
                        .split('&')
                        .filter_map(|p| p.split_once('='))
                        .find(|(k, _)| *k == key)
                        .map(|(_, v)| v.to_string())
                        .unwrap_or_default()
                };
                let body = match arg("action").as_str() {
                    "get_live_categories" => CATS,
                    "get_live_streams" => match arg("category_id").as_str() {
                        "10" => CAT10,
                        "11" => CAT11,
                        _ => ALL,
                    },
                    "get_vod_categories" | "get_series_categories" => "[]",
                    // No action is the login itself.
                    _ => LOGIN,
                };
                let _ = write!(
                    sock,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.flush();
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// Point config, the hand-over file and anything else read through the
    /// environment at a directory of our own, so a test can never see - or
    /// overwrite - the real settings.
    fn isolate(tag: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!("eglass-test-{tag}"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("EagleGlass")).expect("make the test config dir");
        std::env::set_var("APPDATA", &base);
        std::env::set_var("TMP", &base);
        std::env::set_var("TEMP", &base);
        base
    }

    /// Run the frame loop in the order `update` runs it, keeping a trace so a
    /// failure reports what happened rather than only that it did not.
    fn drive(app: &mut App, done: impl Fn(&App) -> bool, max_frames: usize) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut trace = Vec::new();
        for n in 0..max_frames {
            app.pump();
            app.poll_launch_request(&ctx);
            app.follow_launch_in_sidebar();
            trace.push(format!(
                "  frame {n:>3}: cats={} items={} cat={:?} busy={} sel={:?} status={:?}",
                app.cats.len(),
                app.items.len(),
                app.cat,
                app.rx.is_some(),
                app.selected,
                app.status
            ));
            if done(app) {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
        trace
    }

    /// The reported bug: clicking a channel in the guide with the app *not*
    /// running started playback but left the sidebar empty.
    ///
    /// Cold, the login and the launch land in the same frame. The login starts
    /// the channel list and the launch immediately claims the single background
    /// slot for its play job, so the list job's answer is sent to a channel
    /// nobody is reading. The sidebar then has categories but nothing to put
    /// under them.
    #[test]
    fn a_cold_launch_from_the_guide_fills_the_sidebar() {
        let _serial = launch::test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let panel = fake_panel();
        let dir = isolate("cold-launch");
        std::fs::write(
            dir.join("EagleGlass").join("config.txt"),
            format!("server={panel}\nuser=tester\npass=secret\nprofile=\n"),
        )
        .expect("write the test config");

        let mut app = App::default();
        assert!(app.autostart, "the test config should be enough to log in");
        // What main() does with the link when this instance is the first one.
        app.launch_request = Some(launch::PlayRequest {
            id: "2".into(),
            name: "Sky Sports Main Event".into(),
            cat: Some("10".into()),
        });
        app.spawn_login();

        let trace = drive(&mut app, |a| a.items.len() >= 2, 400);
        let when = || trace.join("\n");

        assert!(
            app.cats.iter().any(|c| c.kind == Kind::Live && c.id == "10"),
            "the live categories never arrived:\n{}",
            when()
        );
        assert_eq!(
            app.cat.as_deref(),
            Some("10"),
            "the sidebar did not open the launched channel's category:\n{}",
            when()
        );
        assert!(
            !app.items.is_empty(),
            "the sidebar has no channels in it:\n{}",
            when()
        );
        assert_eq!(
            app.items.len(),
            3,
            "the whole live catalogue should be loaded so other categories stay usable:\n{}",
            when()
        );
        let at = app.selected.expect("the launched channel should be highlighted");
        assert_eq!(app.items[at].id, "2", "the wrong row is highlighted:\n{}", when());
        assert!(
            app.open_cats.contains("10"),
            "the category should be open so the row is visible:\n{}",
            when()
        );
        assert!(
            app.pending_select.is_none() && app.pending_sidebar.is_none(),
            "the launch should be fully applied, not left half-done:\n{}",
            when()
        );
    }

    /// The same click against a window that is already up, which is the case
    /// that was easy to get right - it is here so the cold case cannot regress
    /// back by way of a "fix" to the warm one.
    #[test]
    fn a_launch_into_a_running_window_moves_the_sidebar() {
        let _serial = launch::test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let panel = fake_panel();
        let dir = isolate("warm-launch");
        std::fs::write(
            dir.join("EagleGlass").join("config.txt"),
            format!("server={panel}\nuser=tester\npass=secret\nprofile=\n"),
        )
        .expect("write the test config");

        let mut app = App::default();
        app.spawn_login();
        // Settle first: logged in, the default list loaded.
        drive(&mut app, |a| a.client.is_some() && a.items.len() >= 3, 400);
        assert_eq!(app.items.len(), 3, "expected the whole live list to load");

        // Then the click arrives.
        app.launch_request = Some(launch::PlayRequest {
            id: "3".into(),
            name: "BBC News".into(),
            cat: Some("11".into()),
        });
        let trace = drive(&mut app, |a| a.cat.as_deref() == Some("11") && a.selected.is_some(), 400);

        assert_eq!(app.cat.as_deref(), Some("11"), "wrong category:\n{}", trace.join("\n"));
        assert_eq!(app.items.len(), 3, "the whole list should survive a launch:\n{}", trace.join("\n"));
        let at = app.selected.expect("the launched channel should be highlighted");
        assert_eq!(app.items[at].id, "3");
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut)
}

#[cfg(windows)]
fn detach_console() {    #[link(name = "kernel32")]
    extern "system" {
        fn FreeConsole() -> i32;
    }
    unsafe {
        FreeConsole();
    }
}

#[cfg(not(windows))]
fn detach_console() {}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (server, user, pass, rest) = parse_flags(&args);

    // A link from the TV guide page arrives as eagleglass://play?id=...
    let link = rest.iter().find_map(|a| launch::parse_uri(a));
    let launching = link.is_some();

    if launching || rest.is_empty() {
        // Only the GUI takes part in the single-instance dance. CLI use must stay
        // scriptable, so `eglass.exe presence` never hands off and never exits
        // early.
        if !launch::claim_single_instance() {
            let outcome = match &link {
                Some(req) => launch::send(&launch::Request::Play(req.clone())),
                // A plain second launch: bring the existing window forward
                // instead of leaving the user with nothing happening.
                None => launch::send(&launch::Request::Focus),
            };
            match outcome {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    eprintln!("could not reach the running instance: {e}");
                    std::process::exit(1);
                }
            }
        }
        if let Some(req) = link {
            if let Ok(mut slot) = startup_request().lock() {
                *slot = Some(req);
            }
        }
        // Writes HKCU\Software\Classes\eagleglass so browsers can reach us.
        if let Err(e) = launch::register_uri_scheme() {
            eprintln!("could not register the eagleglass:// link: {e}");
        }
    }

    // The link is the only thing in `rest`, and it has been dealt with - so it
    // must not fall through into CLI mode. That path made a first click, with the
    // app not yet running, print "unknown command" and exit without a window.
    if !rest.is_empty() && !launching {
        if let Err(e) = run_cli(server, user, pass, rest) {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
        return Ok(());
    }

    detach_console();

    // Taskbar/window icon. The exe carries the same artwork as a Win32
    // resource (see build.rs); this is what winit hands to the window itself.
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1600.0, 1030.0])
        .with_min_inner_size([1000.0, 640.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Eagle Glass",
        options,
        Box::new(|cc| {
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(App::default()))
        }),
    )
}

/// A channel the guide page asked us to open, handed over from the command line
/// into `App::default` without threading it through every constructor.
fn startup_request() -> &'static Mutex<Option<launch::PlayRequest>> {
    static SLOT: std::sync::OnceLock<Mutex<Option<launch::PlayRequest>>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}
