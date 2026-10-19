use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use windows::core::{w, BOOL, PCWSTR, PWSTR};
use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::UI::Magnification::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::MonitorInfo;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("ScreenTint");

pub fn init() -> bool {
    unsafe { MagInitialize().as_bool() }
}

pub fn shutdown() {
    set_tint([1.0; 3]);
    unsafe {
        let _ = MagUninitialize();
    }
}

pub fn set_tint(c: [f32; 3]) {
    let mut m = [0f32; 25];
    m[0] = c[0];
    m[6] = c[1];
    m[12] = c[2];
    m[18] = 1.0;
    m[24] = 1.0;
    unsafe {
        let _ = MagSetFullscreenColorEffect(&MAGCOLOREFFECT { transform: m });
    }
}

// ---------------------------------------------------------------- brightness

enum Target {
    /// External monitor over DDC/CI: physical handle (as isize) and its brightness range.
    Ddc { handle: isize, min: u32, max: u32 },
    /// Built-in laptop panel through the `\\.\LCD` video IOCTLs.
    Lcd { levels: Vec<u8> },
}

static TARGETS: Mutex<Vec<Target>> = Mutex::new(Vec::new());

const IOCTL_VIDEO_QUERY_SUPPORTED_BRIGHTNESS: u32 = 0x0023_0496;
const IOCTL_VIDEO_QUERY_DISPLAY_BRIGHTNESS: u32 = 0x0023_0498;
const IOCTL_VIDEO_SET_DISPLAY_BRIGHTNESS: u32 = 0x0023_049C;

fn open_lcd() -> Option<HANDLE> {
    unsafe {
        CreateFileW(
            w!("\\\\.\\LCD"),
            0xC000_0000, // GENERIC_READ | GENERIC_WRITE
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
        .ok()
    }
}

fn scan_lcd() -> Option<(Vec<u8>, i32)> {
    let h = open_lcd()?;
    unsafe {
        let mut levels = [0u8; 256];
        let mut got = 0u32;
        let ok = DeviceIoControl(
            h,
            IOCTL_VIDEO_QUERY_SUPPORTED_BRIGHTNESS,
            None,
            0,
            Some(levels.as_mut_ptr() as *mut c_void),
            256,
            Some(&mut got),
            None,
        )
        .is_ok();
        let mut cur = [0u8; 3]; // policy, AC, DC
        let mut got2 = 0u32;
        let ok2 = DeviceIoControl(
            h,
            IOCTL_VIDEO_QUERY_DISPLAY_BRIGHTNESS,
            None,
            0,
            Some(cur.as_mut_ptr() as *mut c_void),
            3,
            Some(&mut got2),
            None,
        )
        .is_ok();
        let _ = CloseHandle(h);
        if ok && ok2 && got > 0 {
            Some((levels[..got as usize].to_vec(), cur[1] as i32))
        } else {
            None
        }
    }
}

unsafe extern "system" fn collect(hm: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
    let out = &mut *(lp.0 as *mut Vec<HMONITOR>);
    out.push(hm);
    TRUE
}

fn wstr(a: &[u16]) -> String {
    let n = a.iter().position(|&c| c == 0).unwrap_or(a.len());
    String::from_utf16_lossy(&a[..n])
}

/// PnP device ids (like `MONITOR\GSM5B7A\{guid}\0004`) of the monitors attached to an HMONITOR, in order.
fn monitor_device_ids(hm: HMONITOR) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(hm, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
            return out;
        }
        let adapter = mi.szDevice;
        for j in 0.. {
            let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
            if !EnumDisplayDevicesW(PCWSTR(adapter.as_ptr()), j, &mut dd, 0).as_bool() {
                break;
            }
            let id = dd.DeviceID;
            out.push(wstr(&id));
        }
    }
    out
}

fn reg_open(root: HKEY, path: &str) -> Option<HKEY> {
    reg_open_access(root, path, KEY_READ)
}

fn reg_open_access(root: HKEY, path: &str, access: REG_SAM_FLAGS) -> Option<HKEY> {
    let w: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut k = HKEY::default();
    unsafe { RegOpenKeyExW(root, PCWSTR(w.as_ptr()), None, access, &mut k).is_ok().then_some(k) }
}

fn reg_value(k: HKEY, name: &str) -> Option<Vec<u8>> {
    let w: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let mut size = 0u32;
        if RegQueryValueExW(k, PCWSTR(w.as_ptr()), None, None, None, Some(&mut size)).is_err() || size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        RegQueryValueExW(k, PCWSTR(w.as_ptr()), None, None, Some(buf.as_mut_ptr()), Some(&mut size)).is_ok().then_some(buf)
    }
}

/// The monitor's own model name, read from its EDID in the registry.
fn edid_name(device_id: &str) -> Option<String> {
    let parts: Vec<&str> = device_id.split('\\').collect();
    if parts.len() < 4 {
        return None;
    }
    let driver = format!("{}\\{}", parts[2], parts[3]);
    let model_key = reg_open(HKEY_LOCAL_MACHINE, &format!("SYSTEM\\CurrentControlSet\\Enum\\DISPLAY\\{}", parts[1]))?;
    let mut found = None;
    for i in 0.. {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        let rc = unsafe { RegEnumKeyExW(model_key, i, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) };
        if rc.is_err() {
            break;
        }
        let inst = wstr(&name);
        let Some(k) = reg_open(model_key, &inst) else { continue };
        let same = reg_value(k, "Driver")
            .map(|b| {
                let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                wstr(&u).eq_ignore_ascii_case(&driver)
            })
            .unwrap_or(false);
        if same {
            if let Some(p) = reg_open(k, "Device Parameters") {
                found = reg_value(p, "EDID").and_then(|e| parse_edid(&e));
                unsafe {
                    let _ = RegCloseKey(p);
                }
            }
        }
        unsafe {
            let _ = RegCloseKey(k);
        }
        if found.is_some() {
            break;
        }
    }
    unsafe {
        let _ = RegCloseKey(model_key);
    }
    found
}

/// Display product name descriptor (tag 0xFC) of a 128-byte EDID block.
fn parse_edid(e: &[u8]) -> Option<String> {
    if e.len() < 128 {
        return None;
    }
    for off in [54usize, 72, 90, 108] {
        let d = &e[off..off + 18];
        if d[..4] == [0, 0, 0, 0xFC] {
            let text = String::from_utf8_lossy(&d[5..18]);
            let text = text.split('\n').next().unwrap_or("").trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

pub fn scan_monitors() -> Vec<MonitorInfo> {
    let mut found = Vec::new();
    let mut targets = Vec::new();

    if let Some((levels, cur)) = scan_lcd() {
        found.push(MonitorInfo { name: "Tela integrada".into(), percent: cur.clamp(0, 100) });
        targets.push(Target::Lcd { levels });
    }

    unsafe {
        let mut hms: Vec<HMONITOR> = Vec::new();
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut hms as *mut _ as isize));
        for (n, hm) in hms.iter().enumerate() {
            let mut count = 0u32;
            if GetNumberOfPhysicalMonitorsFromHMONITOR(*hm, &mut count).is_err() || count == 0 {
                continue;
            }
            let mut phys = vec![PHYSICAL_MONITOR::default(); count as usize];
            if GetPhysicalMonitorsFromHMONITOR(*hm, &mut phys).is_err() {
                continue;
            }
            let edid = monitor_device_ids(*hm);
            for (j, p) in phys.into_iter().enumerate() {
                let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
                // DDC/CI reads fail now and then (the monitor is busy); retry before giving up on it
                let mut ok = false;
                for attempt in 0..3 {
                    if attempt > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(120));
                    }
                    if GetMonitorBrightness(p.hPhysicalMonitor, &mut min, &mut cur, &mut max) != 0 && max > min {
                        ok = true;
                        break;
                    }
                }
                if !ok {
                    let _ = DestroyPhysicalMonitor(p.hPhysicalMonitor);
                    continue;
                }
                let name = edid
                    .get(j)
                    .and_then(|id| edid_name(id))
                    .unwrap_or_else(|| format!("Monitor {}", n + 1));
                let pct = ((cur - min) * 100 + (max - min) / 2) / (max - min);
                found.push(MonitorInfo { name, percent: pct as i32 });
                targets.push(Target::Ddc { handle: p.hPhysicalMonitor.0 as isize, min, max });
            }
        }
    }

    // two identical models would be indistinguishable in the UI
    for i in 0..found.len() {
        let name = found[i].name.clone();
        if found.iter().filter(|m| m.name == name).count() > 1 {
            let nth = found[..=i].iter().filter(|m| m.name == name).count();
            found[i].name = format!("{name} ({nth})");
        }
    }

    // Old handles are intentionally leaked on a rescan: they are few and tiny.
    *TARGETS.lock().unwrap() = targets;
    found
}

pub fn set_brightness(index: usize, percent: i32) {
    let targets = TARGETS.lock().unwrap();
    let Some(t) = targets.get(index) else { return };
    let percent = percent.clamp(0, 100) as u32;
    unsafe {
        match t {
            Target::Ddc { handle, min, max } => {
                let v = min + (max - min) * percent / 100;
                let _ = SetMonitorBrightness(HANDLE(*handle as *mut c_void), v);
            }
            Target::Lcd { levels } => {
                // snap to the nearest level the panel supports
                let want = percent as i32;
                let lvl = levels.iter().copied().min_by_key(|l| (*l as i32 - want).abs()).unwrap_or(100);
                if let Some(h) = open_lcd() {
                    let buf = [3u8, lvl, lvl]; // DISPLAY_BRIGHTNESS_BOTH, AC, DC
                    let mut got = 0u32;
                    let _ = DeviceIoControl(
                        h,
                        IOCTL_VIDEO_SET_DISPLAY_BRIGHTNESS,
                        Some(buf.as_ptr() as *const c_void),
                        3,
                        None,
                        0,
                        Some(&mut got),
                        None,
                    );
                    let _ = CloseHandle(h);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- misc

pub fn anchor(w: i32, h: i32, _scale: f32) -> (i32, i32) {
    unsafe {
        let mut wa = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut wa as *mut _ as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        (wa.right - w - 12, wa.bottom - h - 12)
    }
}

static BUTTON_WAS_DOWN: AtomicBool = AtomicBool::new(false);

/// True when a mouse button goes down (edge, not level) outside the given rectangle.
/// Used to close the panel without depending on window focus, which is not always granted.
/// Call it on every tick, also while the panel is hidden, so the edge detection stays in sync.
pub fn click_outside(x: i32, y: i32, w: i32, h: i32) -> bool {
    unsafe {
        let down = [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|k| GetAsyncKeyState(k.0 as i32) as u16 & 0x8000 != 0);
        let was_down = BUTTON_WAS_DOWN.swap(down, Ordering::Relaxed);
        if !down || was_down {
            return false;
        }
        let mut p = POINT::default();
        if GetCursorPos(&mut p).is_err() {
            return false;
        }
        p.x < x || p.x >= x + w || p.y < y || p.y >= y + h
    }
}

// ---------------------------------------------------------------- tray icon visibility

const NOTIFY_ICON_SETTINGS: &str = "Control Panel\\NotifyIconSettings";

/// The per-icon settings keys Windows keeps for this executable (they exist once the icon has been shown),
/// as `(key, exact_case_match)`. Entries from older builds with a differently-cased name also match.
fn notify_icon_keys() -> Vec<(HKEY, bool)> {
    let mut found = Vec::new();
    let Ok(exe) = std::env::current_exe() else { return found };
    let exe = exe.to_string_lossy().into_owned();
    let Some(parent) = reg_open(HKEY_CURRENT_USER, NOTIFY_ICON_SETTINGS) else { return found };
    for i in 0.. {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        let rc = unsafe { RegEnumKeyExW(parent, i, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) };
        if rc.is_err() {
            break;
        }
        let Some(k) = reg_open_access(parent, &wstr(&name), KEY_READ | KEY_SET_VALUE) else { continue };
        let path = reg_value(k, "ExecutablePath").map(|b| {
            let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            wstr(&u)
        });
        match path {
            Some(p) if p.eq_ignore_ascii_case(&exe) => found.push((k, p == exe)),
            _ => unsafe {
                let _ = RegCloseKey(k);
            },
        }
    }
    unsafe {
        let _ = RegCloseKey(parent);
    }
    found
}

fn close_all(keys: Vec<(HKEY, bool)>) {
    for (k, _) in keys {
        unsafe {
            let _ = RegCloseKey(k);
        }
    }
}

/// Whether the icon is promoted out of the "hidden icons" overflow. None if Windows has no entry for it yet.
pub fn tray_pin_get() -> Option<bool> {
    let keys = notify_icon_keys();
    let key = keys.iter().find(|(_, exact)| *exact).or(keys.first()).map(|(k, _)| *k);
    let on = key.map(|k| reg_value(k, "IsPromoted").map(|b| b.first().copied().unwrap_or(0) != 0).unwrap_or(false));
    close_all(keys);
    on
}

/// Promotes (or demotes) the icon. This is the same per-icon switch as in Settings > Taskbar > Other system tray icons.
pub fn tray_pin_set(on: bool) {
    let value = (on as u32).to_le_bytes();
    let name: Vec<u16> = "IsPromoted".encode_utf16().chain(Some(0)).collect();
    let keys = notify_icon_keys();
    for (k, _) in &keys {
        unsafe {
            let _ = RegSetValueExW(*k, PCWSTR(name.as_ptr()), None, REG_DWORD, Some(&value));
        }
    }
    close_all(keys);
}

pub fn autostart_get() -> bool {
    unsafe {
        let mut k = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_READ, &mut k).is_err() {
            return false;
        }
        let r = RegQueryValueExW(k, RUN_VALUE, None, None, None, None);
        let _ = RegCloseKey(k);
        r.is_ok()
    }
}

pub fn autostart_set(on: bool) {
    unsafe {
        let mut k = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_WRITE, &mut k).is_err() {
            return;
        }
        if on {
            if let Ok(exe) = std::env::current_exe() {
                let v: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain(Some(0)).collect();
                let bytes = std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2);
                let _ = RegSetValueExW(k, RUN_VALUE, None, REG_SZ, Some(bytes));
            }
        } else {
            let _ = RegDeleteValueW(k, RUN_VALUE);
        }
        let _ = RegCloseKey(k);
    }
}
