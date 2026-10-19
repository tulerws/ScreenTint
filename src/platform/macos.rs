//! macOS backend.
//!
//! Tint: CoreGraphics display transfer formula (per display, no permissions needed).
//! Brightness: the private DisplayServices framework (loaded at runtime) for the built-in panel,
//! and the `m1ddc` command line tool, if installed, for external monitors over DDC/CI.

use std::ffi::{c_char, c_void, CString};
use std::process::Command;
use std::sync::Mutex;

use super::MonitorInfo;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGSetDisplayTransferByFormula(
        display: u32,
        red_min: f32,
        red_max: f32,
        red_gamma: f32,
        green_min: f32,
        green_max: f32,
        green_gamma: f32,
        blue_min: f32,
        blue_max: f32,
        blue_gamma: f32,
    ) -> i32;
    fn CGDisplayRestoreColorSyncSettings();
    fn CGMainDisplayID() -> u32;
    fn CGDisplayPixelsWide(display: u32) -> usize;
    fn CGDisplayIsBuiltin(display: u32) -> u32;
}

extern "C" {
    fn dlopen(path: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

type GetBrightness = unsafe extern "C" fn(u32, *mut f32) -> i32;
type SetBrightness = unsafe extern "C" fn(u32, f32) -> i32;

fn display_services() -> Option<(GetBrightness, SetBrightness)> {
    unsafe {
        let path = CString::new("/System/Library/PrivateFrameworks/DisplayServices.framework/DisplayServices").ok()?;
        let h = dlopen(path.as_ptr(), 1);
        if h.is_null() {
            return None;
        }
        let get = dlsym(h, c"DisplayServicesGetBrightness".as_ptr());
        let set = dlsym(h, c"DisplayServicesSetBrightness".as_ptr());
        if get.is_null() || set.is_null() {
            return None;
        }
        Some((std::mem::transmute::<*mut c_void, GetBrightness>(get), std::mem::transmute::<*mut c_void, SetBrightness>(set)))
    }
}

fn active_displays() -> Vec<u32> {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    unsafe {
        if CGGetActiveDisplayList(16, ids.as_mut_ptr(), &mut n) != 0 {
            return vec![];
        }
    }
    ids[..n as usize].to_vec()
}

pub fn init() -> bool {
    true
}

pub fn shutdown() {
    unsafe { CGDisplayRestoreColorSyncSettings() }
}

pub fn set_tint(c: [f32; 3]) {
    if c == [1.0; 3] {
        unsafe { CGDisplayRestoreColorSyncSettings() };
        return;
    }
    for d in active_displays() {
        unsafe {
            CGSetDisplayTransferByFormula(d, 0.0, c[0], 1.0, 0.0, c[1], 1.0, 0.0, c[2], 1.0);
        }
    }
}

// ---------------------------------------------------------------- brightness

enum Target {
    Builtin(u32),
    M1ddc(u32),
}

static TARGETS: Mutex<Vec<Target>> = Mutex::new(Vec::new());

fn m1ddc() -> Option<&'static str> {
    ["/opt/homebrew/bin/m1ddc", "/usr/local/bin/m1ddc"].into_iter().find(|p| std::path::Path::new(p).exists())
}

pub fn scan_monitors() -> Vec<MonitorInfo> {
    let (mut found, mut targets) = (Vec::new(), Vec::new());

    if let Some((get, _)) = display_services() {
        for d in active_displays() {
            if unsafe { CGDisplayIsBuiltin(d) } == 0 {
                continue;
            }
            let mut v = 0f32;
            if unsafe { get(d, &mut v) } == 0 {
                found.push(MonitorInfo { name: "Tela integrada".into(), percent: (v * 100.0).round() as i32 });
                targets.push(Target::Builtin(d));
            }
        }
    }

    if let Some(bin) = m1ddc() {
        if let Ok(out) = Command::new(bin).args(["display", "list"]).output() {
            // lines like: "[1] LG ULTRAGEAR (3F7B...)"
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let Some(rest) = line.trim().strip_prefix('[') else { continue };
                let Some((idx, name)) = rest.split_once(']') else { continue };
                let Ok(idx) = idx.trim().parse::<u32>() else { continue };
                let get = Command::new(bin).args(["display", &idx.to_string(), "get", "luminance"]).output();
                if let Some(v) = get.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<i32>().ok()) {
                    let name = name.split('(').next().unwrap_or(name).trim();
                    found.push(MonitorInfo { name: name.to_string(), percent: v });
                    targets.push(Target::M1ddc(idx));
                }
            }
        }
    }

    *TARGETS.lock().unwrap() = targets;
    found
}

pub fn set_brightness(index: usize, percent: i32) {
    let targets = TARGETS.lock().unwrap();
    let Some(t) = targets.get(index) else { return };
    let percent = percent.clamp(0, 100);
    match t {
        Target::Builtin(d) => {
            if let Some((_, set)) = display_services() {
                unsafe { set(*d, percent as f32 / 100.0) };
            }
        }
        Target::M1ddc(idx) => {
            if let Some(bin) = m1ddc() {
                let _ = Command::new(bin).args(["display", &idx.to_string(), "set", "luminance", &percent.to_string()]).output();
            }
        }
    }
}

// ---------------------------------------------------------------- misc

pub fn anchor(w: i32, _h: i32, scale: f32) -> (i32, i32) {
    // menu bar items live at the top right
    let screen_w = unsafe { CGDisplayPixelsWide(CGMainDisplayID()) } as f32 * scale;
    ((screen_w as i32) - w - (12.0 * scale) as i32, (32.0 * scale) as i32)
}

fn plist_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/LaunchAgents/com.screentint.app.plist")
}

pub fn autostart_get() -> bool {
    plist_path().exists()
}

pub fn autostart_set(on: bool) {
    let p = plist_path();
    if on {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(
                p,
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>com.screentint.app</string>\n<key>ProgramArguments</key><array><string>{}</string></array>\n<key>RunAtLoad</key><true/>\n</dict></plist>\n",
                    exe.display()
                ),
            );
        }
    } else {
        let _ = std::fs::remove_file(p);
    }
}
