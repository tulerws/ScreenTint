//! Linux backend.
//!
//! Tint: X11 RandR gamma ramps (also covers XWayland-less X sessions). On a pure Wayland session
//! there is no portable way to set gamma, so `init` returns false and the tint is a no-op.
//! Brightness: /sys/class/backlight for laptop panels, `ddcutil` for external monitors.

use std::cell::RefCell;
use std::process::Command;
use std::sync::Mutex;

use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use super::MonitorInfo;

thread_local! {
    static CONN: RefCell<Option<(RustConnection, u32, (u16, u16))>> = const { RefCell::new(None) };
}

fn connect() -> Option<(RustConnection, u32, (u16, u16))> {
    let (conn, n) = RustConnection::connect(None).ok()?;
    let screen = &conn.setup().roots[n];
    let (root, size) = (screen.root, (screen.width_in_pixels, screen.height_in_pixels));
    Some((conn, root, size))
}

fn with_conn<R>(f: impl FnOnce(&RustConnection, u32, (u16, u16)) -> R) -> Option<R> {
    CONN.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = connect();
        }
        c.as_ref().map(|(conn, root, size)| f(conn, *root, *size))
    })
}

pub fn init() -> bool {
    with_conn(|_, _, _| ()).is_some()
}

pub fn shutdown() {
    set_tint([1.0; 3]);
}

pub fn set_tint(c: [f32; 3]) {
    let _ = with_conn(|conn, root, _| -> Result<(), Box<dyn std::error::Error>> {
        let res = conn.randr_get_screen_resources_current(root)?.reply()?;
        for crtc in res.crtcs {
            let n = conn.randr_get_crtc_gamma_size(crtc)?.reply()?.size as usize;
            if n < 2 {
                continue;
            }
            let ramp = |m: f32| -> Vec<u16> {
                (0..n).map(|i| (i as f32 / (n - 1) as f32 * 65535.0 * m) as u16).collect()
            };
            conn.randr_set_crtc_gamma(crtc, &ramp(c[0]), &ramp(c[1]), &ramp(c[2]))?;
        }
        conn.flush()?;
        Ok(())
    });
}

// ---------------------------------------------------------------- brightness

enum Target {
    Backlight { name: String, max: u32 },
    Ddc { display: u32, max: u32 },
}

static TARGETS: Mutex<Vec<Target>> = Mutex::new(Vec::new());

fn read_num(path: &std::path::Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn scan_backlight(found: &mut Vec<MonitorInfo>, targets: &mut Vec<Target>) {
    let Ok(dir) = std::fs::read_dir("/sys/class/backlight") else { return };
    for e in dir.flatten() {
        let p = e.path();
        let (Some(max), Some(cur)) = (read_num(&p.join("max_brightness")), read_num(&p.join("brightness"))) else {
            continue;
        };
        if max == 0 {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        found.push(MonitorInfo { name: "Tela integrada".into(), percent: (cur * 100 / max) as i32 });
        targets.push(Target::Backlight { name, max });
        break; // one internal panel is enough
    }
}

fn scan_ddcutil(found: &mut Vec<MonitorInfo>, targets: &mut Vec<Target>) {
    let Ok(out) = Command::new("ddcutil").args(["detect", "--brief"]).output() else { return };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut display: Option<u32> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(n) = t.strip_prefix("Display ") {
            display = n.trim().parse().ok();
        } else if t.starts_with("Invalid display") {
            display = None;
        } else if let (Some(model), Some(d)) = (t.strip_prefix("Monitor:"), display) {
            // "Monitor:  MFG:Model:Serial"
            let name = model.trim().split(':').nth(1).unwrap_or("Monitor").to_string();
            let Ok(r) = Command::new("ddcutil").args(["--display", &d.to_string(), "getvcp", "10", "--brief"]).output()
            else {
                continue;
            };
            // "VCP 10 C <cur> <max>"
            let s = String::from_utf8_lossy(&r.stdout).to_string();
            let f: Vec<&str> = s.split_whitespace().collect();
            if f.len() >= 5 && f[0] == "VCP" {
                if let (Ok(cur), Ok(max)) = (f[3].parse::<u32>(), f[4].parse::<u32>()) {
                    if max > 0 {
                        found.push(MonitorInfo { name, percent: (cur * 100 / max) as i32 });
                        targets.push(Target::Ddc { display: d, max });
                    }
                }
            }
            display = None;
        }
    }
}

pub fn scan_monitors() -> Vec<MonitorInfo> {
    let (mut found, mut targets) = (Vec::new(), Vec::new());
    scan_backlight(&mut found, &mut targets);
    scan_ddcutil(&mut found, &mut targets);
    *TARGETS.lock().unwrap() = targets;
    found
}

pub fn set_brightness(index: usize, percent: i32) {
    let targets = TARGETS.lock().unwrap();
    let Some(t) = targets.get(index) else { return };
    let percent = percent.clamp(0, 100) as u32;
    match t {
        Target::Backlight { name, max } => {
            let v = (max * percent / 100).max(1);
            let path = format!("/sys/class/backlight/{name}/brightness");
            // writing needs permission (udev rule / video group); fall back to brightnessctl
            if std::fs::write(&path, v.to_string()).is_err() {
                let _ = Command::new("brightnessctl").args(["-d", name, "set", &format!("{percent}%")]).output();
            }
        }
        Target::Ddc { display, max } => {
            let v = max * percent / 100;
            let _ = Command::new("ddcutil")
                .args(["--display", &display.to_string(), "--noverify", "setvcp", "10", &v.to_string()])
                .output();
        }
    }
}

// ---------------------------------------------------------------- misc

pub fn anchor(w: i32, h: i32, _scale: f32) -> (i32, i32) {
    let (sw, sh) = with_conn(|_, _, s| (s.0 as i32, s.1 as i32)).unwrap_or((1920, 1080));
    // panels are usually at the bottom (GNOME: top); leave room for either
    (sw - w - 12, sh - h - 56)
}

fn desktop_file() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let base = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{home}/.config"));
    std::path::PathBuf::from(base).join("autostart/screentint.desktop")
}

pub fn autostart_get() -> bool {
    desktop_file().exists()
}

pub fn autostart_set(on: bool) {
    let p = desktop_file();
    if on {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(
                p,
                format!(
                    "[Desktop Entry]\nType=Application\nName=ScreenTint\nExec=\"{}\"\nX-GNOME-Autostart-enabled=true\n",
                    exe.display()
                ),
            );
        }
    } else {
        let _ = std::fs::remove_file(p);
    }
}

/// Click-outside detection needs a global pointer grab; on X11 the focus-lost event is used instead.
pub fn click_outside(_x: i32, _y: i32, _w: i32, _h: i32) -> bool {
    false
}

/// Not applicable: no per-icon visibility switch outside Windows.
pub fn tray_pin_get() -> Option<bool> {
    None
}

pub fn tray_pin_set(_on: bool) {}
