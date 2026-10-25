<div align="center">

# 🦅 Eagle Glass

**A fast, native desktop IPTV player for Xtream Codes panels — Rust + `egui` + embedded libVLC.**

[![Release](https://img.shields.io/github/v/release/gaultpeter/eagle-glass-friends?style=flat-square&color=8998ff&label=release)](https://github.com/gaultpeter/eagle-glass-friends/releases/latest)
[![Last commit](https://img.shields.io/github/last-commit/gaultpeter/eagle-glass-friends?style=flat-square&label=last%20commit)](https://github.com/gaultpeter/eagle-glass-friends/commits/main)
[![Top language](https://img.shields.io/github/languages/top/gaultpeter/eagle-glass-friends?style=flat-square)](https://github.com/gaultpeter/eagle-glass-friends)
[![License](https://img.shields.io/github/license/gaultpeter/eagle-glass-friends?style=flat-square&color=56d69a)](LICENSE)

[![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-0078D6?style=flat-square&logo=windows)](https://github.com/gaultpeter/eagle-glass-friends/releases)
[![Rust](https://img.shields.io/badge/rust-stable-000000?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![UI](https://img.shields.io/badge/UI-egui-8998ff?style=flat-square)](https://github.com/emilk/egui)
[![Video](https://img.shields.io/badge/video-libVLC-ff8800?style=flat-square&logo=vlcmediaplayer&logoColor=white)](https://www.videolan.org/vlc/libvlc.html)

[**Build from source**](#build-from-source) · [**Presence**](#presence-who-else-is-watching) · [**CLI**](#cli) · [**Upstream**](https://github.com/gaultpeter/eagle-glass)

</div>

---

Eagle Glass is a **power-user-first** live-TV player: a small, snappy native window that logs into your
**Xtream Codes** panel, browses the whole catalogue, and plays channels with an **embedded libVLC**
surface (video renders *inside* the app - no separate VLC window popping up).

> [!NOTE]
> This is the **friends build** of Eagle Glass: the same player plus a **"who is watching"** presence
> feature for a shared Xtream line. Pick your name in the wizard and the top bar tells you who else is
> on the account, backed by the [`eglass-presence`](https://github.com/gaultpeter/eglass-presence)
> Cloudflare Worker. See [Presence](#presence-who-else-is-watching).

## Features

- **Browsable tree** - `Section -> category -> channel` in the sidebar, with per-category channel counts.
- **Embedded player** - video renders inside the app via a libVLC child surface; switching channels replaces the stream in place.
- **True 16:9** player pane (letterboxed when needed).
- **Sleek controls** - play/pause, volume, fullscreen - with a red **LIVE** badge.
- **Video-only fullscreen** - `F11`, the button, or **double-click** the video (double-click again to exit).
- **Keyboard driven** - `Space` play/pause, `M` mute, `F` fullscreen, `Up`/`Down` volume.
- **First-run wizard** + connection **pill** and a docked settings **panel**; credentials are remembered.
- **Identity is set once** - in the wizard, or changed deliberately in settings. There is nothing to switch mid-session.
- **Settings stay out of the way** - once the panel accepts your credentials they collapse to a summary, leaving only the things worth touching (live format, picture) in view.
- **Who's watching** - see which of your friends is on the shared account, what they're watching, and whether the connection limit is about to bite.
- **CLI mode** for scripting (`categories`, `list`, `url`, `m3u`, `epg`).
- Clean dark theme, no clutter.

## Download

Grab `eglass.exe` from the [**Releases**](https://github.com/gaultpeter/eagle-glass-friends/releases/latest)
page, or build it yourself below.

> [!TIP]
> Releases are built by GitHub Actions and published with a SHA-256 checksum. Check both before running:
>
> ```powershell
> Get-FileHash .\eglass.exe -Algorithm SHA256
> Get-AuthenticodeSignature .\eglass.exe | Select-Object Status, SignerCertificate
> ```
>
> The hash must match `SHA256SUMS.txt` on the release page. See [docs/SIGNING.md](docs/SIGNING.md) for what the signature does and does not prove.

Free code signing provided by [SignPath.io](https://signpath.io/), certificate by [SignPath Foundation](https://signpath.org/). See [docs/SIGNING.md](docs/SIGNING.md) and [PRIVACY.md](PRIVACY.md).

> [!IMPORTANT]
> **64-bit VLC must be installed** (Eagle Glass loads `libvlc.dll` at runtime - it does not bundle it).
> Install [VLC](https://www.videolan.org/vlc/) or point `EG_VLC_DIR` at a folder containing `libvlc.dll`.

## Quick start

1. Run `eglass.exe`.
2. On first launch a **welcome wizard** asks for your panel **Server**, **Username** and **Password**,
   plus **who is watching** (your name). They're saved to `%APPDATA%\EagleGlass\config.txt` and reused
   automatically next time.
3. Pick a category in the sidebar, expand it, and click a channel to play.
4. Double-click the video (or press `F11`) for fullscreen.
5. The top bar shows **who else is on the shared account**. Hover it for the detail, click it for the
   phone dashboard.

> [!WARNING]
> The saved config file is **plaintext**. Treat `%APPDATA%\EagleGlass\config.txt` like a secret - it
> holds your panel password *and* the shared presence token. Don't commit or share it.

## Build from source

**Requirements:** Windows 10/11, [Rust](https://rustup.rs/) (stable), 64-bit VLC installed.
(Visual Studio C++ Build Tools are needed for the MSVC toolchain.)

```bash
git clone https://github.com/gaultpeter/eagle-glass-friends.git
cd eagle-glass-friends
cargo build --release
```

Output: `target\release\eglass.exe`

## Keyboard shortcuts

| Key | Action |
|-----|--------|
| `Space` | Play / pause |
| `M` | Mute / unmute |
| `F` | Toggle fullscreen |
| `Esc` | Exit fullscreen |
| `Up` / `Down` | Volume up / down |
| double-click video | Toggle fullscreen |

## Presence (who else is watching)

Four people sharing one Xtream line, and the line allows **one connection at a time**, means somebody
eventually gets kicked mid-match. The panel will not tell you who is on - its public API
(`player_api.php`) only reports a **count** of live connections, never who or what. So this build asks
each other instead.

### The guard

The important part is not the pill, it is what happens when you press play.

If you are **not already watching**, the app asks the panel for `active_cons` (the authority on whether
the line is free) and the worker for the roster (the authority on *who*). Then:

| Panel says | You see |
|---|---|
| Free | Nothing. The stream just starts - the common case stays zero-friction. |
| Busy, a friend is checked in | *"Blain is watching Sky Sports Main Event · 38m in. This line allows 1 connection. Starting will kick them off."* with **Cancel** / **Take the line**. |
| Busy, only your own other device | *"You're already watching on another device."* |
| Busy, nobody checked in | Nothing blocks. You get a status note: *"Panel reports 1/1 in use with nobody checked in - taking the line."* |

That last row is deliberate. The panel's `active_cons` is only a hint: measured
against a real panel it stayed at 1 for over four minutes with nothing running at
all, because the counter is slow to clear. A dialog on that reading would fire on
every single play attempt. The rule is therefore narrow - **stop only when we can
name somebody we would displace** - which also means a stranger's connection costs
you nothing but a note, and they were not authorised anyway.

Once a stream is up you hold the connection, so zapping channels never re-prompts.

If the worker cannot be reached the guard does not invent a warning: it plays.

### The pill

The top bar answers "can I watch, and if not, who has it" - not "how many connections are in use",
which on a one-connection line is true whenever you are happily watching your own channel.

| | |
|---|---|
| `Line free` | green - nobody is on the account |
| `Blain · Sky Sports F1` | who has it, and what they are watching |
| `Line in use · unknown` | the panel counts a connection that no client of ours claims |
| `Conflict · Blain also on` | two of us on a one-connection line - one is about to be cut |
| `Line is yours` | green - you are the one watching |

Hover for each friend's channel, how long they've been on, and their device. Click for the phone
dashboard.

### Being kicked off

If your stream stops and you did not pause it, the app asks for a fresh roster and tells you who took
the line: *"Blain took the line - now on Sky Sports Main Event"*. It would be poor form to let your
stream die unexplained. A pause is treated as deliberate, so pausing never triggers this.

While a stream is up the app heartbeats the worker every 90s. A paused stream still holds a panel
connection, so pausing keeps you on the roster. Presence clears when you exit, and self-expires on the
worker within ~3 minutes if the app is killed.

### Setup

Paste the worker URL and shared token into the settings panel (the gear, top right), or add them to
`%APPDATA%\EagleGlass\config.txt`:

```ini
server=http://your-panel:8080
user=shared
pass=shared
format=ts
profile=Peter
device_id=9f3c1a2b7d4e5f60
presence_url=https://eglass-presence.your-subdomain.workers.dev
presence_token=<the PRESENCE_TOKEN secret>
```

`device_id` is generated on first run and must be left alone - it is the roster key suffix. If you
watch on a second machine, leave its own generated id as-is; both show up, which is the point, since
each one consumes a connection.

**Reset app…** at the bottom of the settings panel deletes `%APPDATA%\EagleGlass\config.txt` and
returns to the first-run wizard. It clears the presence entry first, so the others stop seeing a
watcher that no longer exists.

## Environment variables

| Variable | Purpose |
|----------|---------|
| `EG_SERVER` / `EG_USER` / `EG_PASS` | Override the saved connection (auto-connects when user + pass are set). |
| `EG_FORMAT` | Override the saved live format (`ts` / `m3u8`). |
| `EG_PROFILE` | Override who is watching, e.g. `Peter`. |
| `EG_PRESENCE_URL` / `EG_PRESENCE_TOKEN` | Override the presence worker and its shared token. |
| `EG_AUTOPLAY` | Stream id to auto-play once the Live list loads. |
| `EG_SETTINGS` | Open the settings panel at launch (handy for scripting and screenshots). |
| `EG_VLC_DIR` | Folder containing `libvlc.dll` (defaults to the installed VLC). |

## CLI

```
eglass.exe categories
eglass.exe list live [category_id]
eglass.exe url  live <stream_id>
eglass.exe play live <stream_id>     # CLI play uses an external VLC process
eglass.exe m3u playlist.m3u
eglass.exe epg
```

## How the video works

`src/video.rs` loads `libvlc.dll` at runtime (via [`libloading`](https://crates.io/crates/libloading)),
creates a child Win32 window inside the app window, and points libVLC at it with
`libvlc_media_player_set_hwnd`. The child window is repositioned every frame to match the `egui`
video rectangle. Because that surface is a native child window, `egui` can't paint over it - which is
why the settings form is a **docked panel** rather than a floating dialog: it is laid out beside the
video, so the video rectangle simply gets narrower and playback never has to be interrupted. The
pre-play guard is the one thing that still shrinks the video away, and that only appears when nothing
is playing.

## Project layout

| File | Purpose |
|------|---------|
| `src/xtream.rs` | Xtream client: login, categories, items, episodes, URL builders, M3U/EPG |
| `src/video.rs` | libVLC FFI + embedded Win32 child window |
| `src/presence.rs` | Presence client + background heartbeat thread for the shared line |
| `src/main.rs` | `egui` GUI + CLI |

## Notes & limitations

- **Windows only** - the embedded video relies on a Win32 child window.
- Requires a **64-bit VLC** install for `libvlc.dll`.
- Blocking HTTP (`ureq`) runs on worker threads; the UI stays responsive. Presence is one more such
  thread and never blocks the UI.
- `max_connections` is enforced by your panel; on many panels concurrent streams from one IP count as one.
- Live streams have no seek bar (no timeshift).
- **Presence is cooperative and advisory.** It knows only about people running this build. The token is
  compiled into the exe and can be extracted by whoever holds it - the worker validates profile names
  server-side, so nobody can forge a *name* into the roster, but they could forge a heartbeat.
  The unaccounted-watcher warning is the backstop.
- **The guard cannot eliminate the race.** If you and Blain press play within a few seconds of each
  other, you both see "free" and one of you gets cut. A short claim on the worker would shrink that
  window but not close it.
- The roster refreshes every 20s while you are watching and every 60s while idle; heartbeats are
  written every 90s to stay inside Cloudflare KV's free write allowance (measured at 48 writes and
  960 reads per hour of watching, with zero list calls - see the worker's README for the budget).
- `cargo test` covers the guard's decision logic and the pill's wording, including the one-connection
  cases - those are pure functions precisely so they can be tested without a window or a panel.

## License

[MIT](LICENSE) © 2026 Peter Gault
