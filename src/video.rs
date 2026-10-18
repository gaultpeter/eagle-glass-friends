use std::ffi::{c_char, c_int, c_void, CString};
use std::path::Path;
use std::ptr;

type Handle = *mut c_void;

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct RectI {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

const VK_LBUTTON: c_int = 0x01;
const GA_ROOT: u32 = 2;

pub fn left_button_down() -> bool {
    unsafe { (GetAsyncKeyState(VK_LBUTTON) as u16 & 0x8000) != 0 }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[repr(C)]
struct WndClassW {
    style: u32,
    lpfn_wnd_proc: Option<unsafe extern "system" fn(Handle, u32, usize, isize) -> isize>,
    cb_cls_extra: c_int,
    cb_wnd_extra: c_int,
    h_instance: Handle,
    h_icon: Handle,
    h_cursor: Handle,
    h_background: Handle,
    lpsz_menu_name: *const u16,
    lpsz_class_name: *const u16,
}

const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
const SW_SHOW: c_int = 5;

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(wc: *const WndClassW) -> u16;
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: c_int,
        y: c_int,
        w: c_int,
        h: c_int,
        parent: Handle,
        menu: Handle,
        inst: Handle,
        param: *mut c_void,
    ) -> Handle;
    fn DefWindowProcW(hwnd: Handle, msg: u32, w: usize, l: isize) -> isize;
    fn MoveWindow(hwnd: Handle, x: c_int, y: c_int, w: c_int, h: c_int, repaint: c_int) -> c_int;
    fn ShowWindow(hwnd: Handle, cmd: c_int) -> c_int;
    fn DestroyWindow(hwnd: Handle) -> c_int;
    fn GetAsyncKeyState(vkey: c_int) -> i16;
    fn GetCursorPos(p: *mut Point) -> c_int;
    fn ScreenToClient(hwnd: Handle, p: *mut Point) -> c_int;
    fn GetClientRect(hwnd: Handle, r: *mut RectI) -> c_int;
    fn GetForegroundWindow() -> Handle;
    fn WindowFromPoint(p: Point) -> Handle;
    fn GetAncestor(hwnd: Handle, flags: u32) -> Handle;
    fn IsChild(parent: Handle, child: Handle) -> c_int;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn SetDllDirectoryW(path: *const u16) -> c_int;
}

#[link(name = "gdi32")]
extern "system" {
    fn GetStockObject(i: c_int) -> Handle;
}

const BLACK_BRUSH: c_int = 4;

unsafe extern "system" fn wnd_proc(hwnd: Handle, msg: u32, w: usize, l: isize) -> isize {
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

struct Lib {
    _lib: libloading::Library,
    new: unsafe extern "C" fn(c_int, *const *const c_char) -> Handle,
    release: unsafe extern "C" fn(Handle),
    media_new_location: unsafe extern "C" fn(Handle, *const c_char) -> Handle,
    media_release: unsafe extern "C" fn(Handle),
    mp_new: unsafe extern "C" fn(Handle) -> Handle,
    mp_set_media: unsafe extern "C" fn(Handle, Handle),
    mp_set_hwnd: unsafe extern "C" fn(Handle, Handle),
    mp_play: unsafe extern "C" fn(Handle) -> c_int,
    mp_pause: unsafe extern "C" fn(Handle),
    mp_stop: unsafe extern "C" fn(Handle),
    mp_is_playing: unsafe extern "C" fn(Handle) -> c_int,
    audio_set_volume: unsafe extern "C" fn(Handle, c_int) -> c_int,
    audio_get_volume: unsafe extern "C" fn(Handle) -> c_int,
    video_set_adjust_int: unsafe extern "C" fn(Handle, c_int, c_int) -> c_int,
    video_set_adjust_float: unsafe extern "C" fn(Handle, c_int, f32) -> c_int,
    mp_release: unsafe extern "C" fn(Handle),
}

pub const ADJ_ENABLE: c_int = 0;
pub const ADJ_CONTRAST: c_int = 1;
pub const ADJ_BRIGHTNESS: c_int = 2;
pub const ADJ_GAMMA: c_int = 5;

fn load_lib() -> Result<Lib, String> {
    let mut dirs: Vec<String> = Vec::new();
    if let Ok(d) = std::env::var("EG_VLC_DIR") {
        if !d.is_empty() {
            dirs.push(d);
        }
    }
    if let Some(d) = vlc_install_dir() {
        dirs.push(d);
    }
    dirs.push(r"C:\Program Files\VideoLAN\VLC".to_string());
    dirs.push(r"C:\Program Files (x86)\VideoLAN\VLC".to_string());

    for dir in dirs {
        let dll = format!("{}\\libvlc.dll", dir);
        if !Path::new(&dll).exists() {
            continue;
        }
        unsafe {
            SetDllDirectoryW(wide(&dir).as_ptr());
        }
        unsafe {
            let lib = libloading::Library::new(&dll).map_err(|e| e.to_string())?;
            let new = *lib.get::<unsafe extern "C" fn(c_int, *const *const c_char) -> Handle>(b"libvlc_new\0").map_err(|e| e.to_string())?;
            let release = *lib.get::<unsafe extern "C" fn(Handle)>(b"libvlc_release\0").map_err(|e| e.to_string())?;
            let media_new_location = *lib.get::<unsafe extern "C" fn(Handle, *const c_char) -> Handle>(b"libvlc_media_new_location\0").map_err(|e| e.to_string())?;
            let media_release = *lib.get::<unsafe extern "C" fn(Handle)>(b"libvlc_media_release\0").map_err(|e| e.to_string())?;
            let mp_new = *lib.get::<unsafe extern "C" fn(Handle) -> Handle>(b"libvlc_media_player_new\0").map_err(|e| e.to_string())?;
            let mp_set_media = *lib.get::<unsafe extern "C" fn(Handle, Handle)>(b"libvlc_media_player_set_media\0").map_err(|e| e.to_string())?;
            let mp_set_hwnd = *lib.get::<unsafe extern "C" fn(Handle, Handle)>(b"libvlc_media_player_set_hwnd\0").map_err(|e| e.to_string())?;
            let mp_play = *lib.get::<unsafe extern "C" fn(Handle) -> c_int>(b"libvlc_media_player_play\0").map_err(|e| e.to_string())?;
            let mp_pause = *lib.get::<unsafe extern "C" fn(Handle)>(b"libvlc_media_player_pause\0").map_err(|e| e.to_string())?;
            let mp_stop = *lib.get::<unsafe extern "C" fn(Handle)>(b"libvlc_media_player_stop\0").map_err(|e| e.to_string())?;
            let mp_is_playing = *lib.get::<unsafe extern "C" fn(Handle) -> c_int>(b"libvlc_media_player_is_playing\0").map_err(|e| e.to_string())?;
            let audio_set_volume = *lib.get::<unsafe extern "C" fn(Handle, c_int) -> c_int>(b"libvlc_audio_set_volume\0").map_err(|e| e.to_string())?;
            let audio_get_volume = *lib.get::<unsafe extern "C" fn(Handle) -> c_int>(b"libvlc_audio_get_volume\0").map_err(|e| e.to_string())?;
            let video_set_adjust_int = *lib.get::<unsafe extern "C" fn(Handle, c_int, c_int) -> c_int>(b"libvlc_video_set_adjust_int\0").map_err(|e| e.to_string())?;
            let video_set_adjust_float = *lib.get::<unsafe extern "C" fn(Handle, c_int, f32) -> c_int>(b"libvlc_video_set_adjust_float\0").map_err(|e| e.to_string())?;
            let mp_release = *lib.get::<unsafe extern "C" fn(Handle)>(b"libvlc_media_player_release\0").map_err(|e| e.to_string())?;
            return Ok(Lib {
                _lib: lib,
                new,
                release,
                media_new_location,
                media_release,
                mp_new,
                mp_set_media,
                mp_set_hwnd,
                mp_play,
                mp_pause,
                mp_stop,
                mp_is_playing,
                audio_set_volume,
                audio_get_volume,
                video_set_adjust_int,
                video_set_adjust_float,
                mp_release,
            });
        }
    }
    Err("libvlc.dll not found. Install 64-bit VLC or set EG_VLC_DIR to its folder".into())
}

fn vlc_install_dir() -> Option<String> {
    if let Ok(p) = std::env::var("EG_VLC") {
        let path = Path::new(&p);
        if let Some(parent) = path.parent() {
            if parent.join("libvlc.dll").exists() {
                return Some(parent.to_string_lossy().to_string());
            }
        }
    }
    for d in [r"C:\Program Files\VideoLAN\VLC", r"C:\Program Files (x86)\VideoLAN\VLC"] {
        if Path::new(d).join("libvlc.dll").exists() {
            return Some(d.to_string());
        }
    }
    None
}

pub struct Player {
    lib: Option<Lib>,
    inst: Handle,
    mp: Handle,
    child: Handle,
    parent: Handle,
    vol: c_int,
}

impl Player {
    pub fn new() -> Self {
        Player {
            lib: None,
            inst: ptr::null_mut(),
            mp: ptr::null_mut(),
            child: ptr::null_mut(),
            parent: ptr::null_mut(),
            vol: 100,
        }
    }

    pub fn set_parent(&mut self, hwnd: Handle) {
        if self.parent.is_null() {
            self.parent = hwnd;
        }
    }

    fn ensure(&mut self) -> Result<(), String> {
        if self.lib.is_none() {
            let lib = load_lib()?;
            let args = ["--quiet", "--no-video-title-show", "--network-caching=1500"]
                .iter()
                .map(|s| CString::new(*s).unwrap())
                .collect::<Vec<_>>();
            let ptrs = args.iter().map(|c| c.as_ptr()).collect::<Vec<_>>();
            let inst = unsafe { (lib.new)(ptrs.len() as c_int, ptrs.as_ptr()) };
            if inst.is_null() {
                return Err("libvlc_new failed".into());
            }
            self.inst = inst;
            self.mp = unsafe { (lib.mp_new)(inst) };
            self.lib = Some(lib);
        }
        Ok(())
    }

    fn ensure_child(&mut self) -> Result<(), String> {
        if !self.child.is_null() {
            return Ok(());
        }
        unsafe {
            let parent = self.parent;
            if parent.is_null() {
                return Err("app window handle not set yet".into());
            }
            let cls = wide("EglassVideo");
            let inst = GetModuleHandleW(ptr::null());
            let wc = WndClassW {
                style: 0,
                lpfn_wnd_proc: Some(wnd_proc),
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance: inst,
                h_icon: ptr::null_mut(),
                h_cursor: ptr::null_mut(),
                h_background: GetStockObject(BLACK_BRUSH),
                lpsz_menu_name: ptr::null(),
                lpsz_class_name: cls.as_ptr(),
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                0,
                cls.as_ptr(),
                wide("video").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
                0,
                0,
                16,
                16,
                parent,
                ptr::null_mut(),
                inst,
                ptr::null_mut(),
            );
            if hwnd.is_null() {
                return Err("CreateWindowExW failed".into());
            }
            self.child = hwnd;
        }
        Ok(())
    }

    pub fn set_bounds(&mut self, x: f32, y: f32, w: f32, h: f32) {
        if self.ensure_child().is_err() {
            return;
        }
        unsafe {
            MoveWindow(self.child, x as c_int, y as c_int, w.max(1.0) as c_int, h.max(1.0) as c_int, 1);
        }
    }

    pub fn play(&mut self, url: &str) -> Result<(), String> {
        self.ensure()?;
        self.ensure_child()?;
        let lib = self.lib.as_ref().unwrap();
        let mrl = CString::new(url).map_err(|_| "invalid url".to_string())?;
        unsafe {
            (lib.mp_stop)(self.mp);
            let media = (lib.media_new_location)(self.inst, mrl.as_ptr());
            if media.is_null() {
                return Err("could not create media".into());
            }
            (lib.mp_set_media)(self.mp, media);
            (lib.media_release)(media);
            (lib.mp_set_hwnd)(self.mp, self.child);
            ShowWindow(self.child, SW_SHOW);
            (lib.mp_play)(self.mp);
            (lib.audio_set_volume)(self.mp, self.vol);
        }
        Ok(())
    }

    pub fn is_playing(&self) -> bool {
        match &self.lib {
            Some(lib) if !self.mp.is_null() => unsafe { (lib.mp_is_playing)(self.mp) != 0 },
            _ => false,
        }
    }

    pub fn pause_toggle(&self) {
        if let Some(lib) = &self.lib {
            if !self.mp.is_null() {
                unsafe { (lib.mp_pause)(self.mp) };
            }
        }
    }

    #[allow(dead_code)]
    pub fn stop(&self) {
        if let Some(lib) = &self.lib {
            if !self.mp.is_null() {
                unsafe { (lib.mp_stop)(self.mp) };
            }
        }
    }

    pub fn set_volume(&mut self, v: c_int) {
        self.vol = v.clamp(0, 100);
        if let Some(lib) = &self.lib {
            if !self.mp.is_null() {
                unsafe { (lib.audio_set_volume)(self.mp, self.vol) };
            }
        }
    }

    pub fn cursor_over_video(&self) -> bool {
        if self.child.is_null() || self.parent.is_null() {
            return false;
        }
        unsafe {
            let mut pt = Point { x: 0, y: 0 };
            if GetCursorPos(&mut pt) == 0 {
                return false;
            }
            // the app must be the active (foreground) window
            let root = GetAncestor(self.parent, GA_ROOT);
            if root.is_null() || GetForegroundWindow() != root {
                return false;
            }
            // the topmost window actually under the cursor must belong to our app
            // (i.e. the video isn't covered by another window)
            let under = WindowFromPoint(Point { x: pt.x, y: pt.y });
            if under.is_null() || IsChild(self.parent, under) == 0 {
                return false;
            }
            // and the cursor must be within the video surface
            let mut p2 = Point { x: pt.x, y: pt.y };
            if ScreenToClient(self.child, &mut p2) == 0 {
                return false;
            }
            let mut rc = RectI { left: 0, top: 0, right: 0, bottom: 0 };
            if GetClientRect(self.child, &mut rc) == 0 {
                return false;
            }
            p2.x >= 0 && p2.y >= 0 && p2.x < rc.right && p2.y < rc.bottom
        }
    }

    pub fn set_picture(&self, brightness: f32, contrast: f32, gamma: f32) {
        if let Some(lib) = &self.lib {
            if !self.mp.is_null() {
                unsafe {
                    (lib.video_set_adjust_int)(self.mp, ADJ_ENABLE, 1);
                    (lib.video_set_adjust_float)(self.mp, ADJ_BRIGHTNESS, brightness);
                    (lib.video_set_adjust_float)(self.mp, ADJ_CONTRAST, contrast);
                    (lib.video_set_adjust_float)(self.mp, ADJ_GAMMA, gamma);
                }
            }
        }
    }

    pub fn volume(&self) -> c_int {
        self.vol
    }

    #[allow(dead_code)]
    pub fn current_volume(&self) -> c_int {
        if let Some(lib) = &self.lib {
            if !self.mp.is_null() {
                let v = unsafe { (lib.audio_get_volume)(self.mp) };
                if v >= 0 {
                    return v;
                }
            }
        }
        self.vol
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        unsafe {
            if let Some(lib) = &self.lib {
                if !self.mp.is_null() {
                    (lib.mp_stop)(self.mp);
                    (lib.mp_release)(self.mp);
                }
                if !self.inst.is_null() {
                    (lib.release)(self.inst);
                }
            }
            if !self.child.is_null() {
                DestroyWindow(self.child);
            }
        }
    }
}
