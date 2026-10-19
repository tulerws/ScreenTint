//! OS backends. Each one provides the same free functions:
//!
//! - `init() -> bool` / `shutdown()`
//! - `set_tint([r, g, b])`: multiply the whole screen by a per-channel factor (1.0 = untouched)
//! - `scan_monitors() -> Vec<MonitorInfo>`: monitors whose hardware brightness can be controlled
//! - `set_brightness(index, percent)`: index into the last `scan_monitors` result
//! - `anchor(w, h, scale) -> (x, y)`: where to put the panel (physical px), next to the tray
//! - `autostart_get() -> bool` / `autostart_set(bool)`

pub struct MonitorInfo {
    pub name: String,
    pub percent: i32,
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use self::linux::*;
