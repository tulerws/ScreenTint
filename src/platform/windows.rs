use std::ffi::c_void;
use std::sync::Mutex;

use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::*;
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
            for p in phys {
                let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
                if GetMonitorBrightness(p.hPhysicalMonitor, &mut min, &mut cur, &mut max) == 0 || max <= min {
                    let _ = DestroyPhysicalMonitor(p.hPhysicalMonitor);
                    continue;
                }
                let raw = p.szPhysicalMonitorDescription; // copy: the struct is packed
                let len = raw.iter().position(|&c| c == 0).unwrap_or(0);
                let desc = String::from_utf16_lossy(&raw[..len]);
                let name = if desc.is_empty() || desc.starts_with("Generic") {
                    format!("Monitor {}", n + 1)
                } else {
                    format!("{} ({})", desc, n + 1)
                };
                let pct = ((cur - min) * 100 + (max - min) / 2) / (max - min);
                found.push(MonitorInfo { name, percent: pct as i32 });
                targets.push(Target::Ddc { handle: p.hPhysicalMonitor.0 as isize, min, max });
            }
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
