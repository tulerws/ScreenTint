//! Platform-independent logic: settings, colour math, schedule, persistence.

use std::path::PathBuf;

use chrono::Timelike;

pub const TEMP_MIN: i32 = 1200;
pub const TEMP_MAX: i32 = 6500;
pub const BRI_MIN: i32 = 10;

#[derive(Clone, Copy, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub color_mode: bool,
    pub temp: i32,
    pub hue: i32,
    pub sat: i32,
    pub strength: i32,
    pub bright: i32,
    pub sched: bool,
    pub t1: i32,
    pub t2: i32,
    pub link: bool,
    pub pinned: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            color_mode: false,
            temp: 3600,
            hue: 28,
            sat: 85,
            strength: 100,
            bright: 100,
            sched: false,
            t1: 21 * 60,
            t2: 7 * 60,
            link: true,
            pinned: false,
        }
    }
}

pub fn now_minutes() -> i32 {
    let t = chrono::Local::now().time();
    t.hour() as i32 * 60 + t.minute() as i32
}

pub fn kelvin_to_rgb(k: i32) -> [f32; 3] {
    let t = k as f32 / 100.0;
    let r = if t <= 66.0 { 255.0 } else { 329.698_73 * (t - 60.0).powf(-0.133_204_76) };
    let g = if t <= 66.0 {
        99.470_8 * t.ln() - 161.119_57
    } else {
        288.122_17 * (t - 60.0).powf(-0.075_514_85)
    };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };
    [r, g, b].map(|c: f32| c.clamp(0.0, 255.0) / 255.0)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

impl Settings {
    pub fn active(&self) -> bool {
        self.enabled && (!self.sched || self.in_window(now_minutes()))
    }

    fn in_window(&self, now: i32) -> bool {
        if self.t1 <= self.t2 {
            now >= self.t1 && now < self.t2
        } else {
            now >= self.t1 || now < self.t2
        }
    }

    /// Colour of the tint before intensity is applied.
    pub fn base_color(&self) -> [f32; 3] {
        if self.color_mode {
            hsv_to_rgb(self.hue as f32, self.sat as f32 / 100.0, 1.0)
        } else {
            kelvin_to_rgb(self.temp)
        }
    }

    /// Tint colour after intensity (what the screen is multiplied by, before brightness).
    pub fn tint_color(&self) -> [f32; 3] {
        let k = self.strength as f32 / 100.0;
        self.base_color().map(|c| 1.0 + (c - 1.0) * k)
    }

    /// Per-channel multiplier applied to the whole screen.
    pub fn target(&self) -> [f32; 3] {
        if !self.active() {
            return [1.0; 3];
        }
        let b = self.bright as f32 / 100.0;
        self.tint_color().map(|c| c * b)
    }
}

// ---------------------------------------------------------------- persistence

pub fn config_dir() -> PathBuf {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if cfg!(windows) {
        PathBuf::from(var("APPDATA").unwrap_or_else(|| ".".into())).join("ScreenTint")
    } else if cfg!(target_os = "macos") {
        PathBuf::from(var("HOME").unwrap_or_else(|| ".".into()))
            .join("Library/Application Support/ScreenTint")
    } else {
        var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| var("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| ".".into())
            .join("screentint")
    }
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.ini")
}

pub fn load() -> Settings {
    let mut s = Settings::default();
    let Ok(text) = std::fs::read_to_string(settings_path()) else {
        return s;
    };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let n = v.trim().parse::<i32>().unwrap_or(0);
        match k.trim() {
            "enabled" => s.enabled = n != 0,
            "color_mode" => s.color_mode = n != 0,
            "temp" => s.temp = n.clamp(TEMP_MIN, TEMP_MAX),
            "hue" => s.hue = n.clamp(0, 360),
            "sat" => s.sat = n.clamp(0, 100),
            "strength" => s.strength = n.clamp(0, 100),
            "bright" => s.bright = n.clamp(BRI_MIN, 100),
            "sched" => s.sched = n != 0,
            "t1" => s.t1 = n.clamp(0, 1439),
            "t2" => s.t2 = n.clamp(0, 1439),
            "link" => s.link = n != 0,
            "pinned" => s.pinned = n != 0,
            _ => {}
        }
    }
    s
}

pub fn save(s: &Settings) {
    let text = format!(
        "enabled={}\ncolor_mode={}\ntemp={}\nhue={}\nsat={}\nstrength={}\nbright={}\nsched={}\nt1={}\nt2={}\nlink={}\npinned={}\n",
        s.enabled as i32, s.color_mode as i32, s.temp, s.hue, s.sat, s.strength, s.bright,
        s.sched as i32, s.t1, s.t2, s.link as i32, s.pinned as i32
    );
    let _ = std::fs::create_dir_all(config_dir());
    let _ = std::fs::write(settings_path(), text);
}
