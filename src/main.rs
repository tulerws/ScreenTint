#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod model;
mod platform;

use std::cell::RefCell;
use std::net::{TcpListener, TcpStream};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, ModelRc, PhysicalPosition, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel};

use model::*;

slint::include_modules!();

const PORT: u16 = 47831;
static SHOW_REQ: AtomicBool = AtomicBool::new(false);

enum Cmd {
    Scan,
    Set(usize, i32),
}

struct State {
    s: Settings,
    names: Vec<String>,
    mon: Vec<f32>,
    offsets: Vec<f32>,
    master: f32,
    scanning: bool,
    cur: [f32; 3],
    last_hide: Option<Instant>,
    placed: (u32, u32),
}

struct Ctx {
    panel: slint::Weak<Panel>,
    tray: slint::Weak<Tray>,
    st: RefCell<State>,
    tx: Sender<Cmd>,
    anim: Timer,
    save: Timer,
}

// ---------------------------------------------------------------- hardware worker

/// Talks to the monitors off the UI thread. DDC/CI is slow (50-200 ms per write), so writes
/// are coalesced: only the latest value per monitor is sent.
fn worker(rx: Receiver<Cmd>, res: Sender<Vec<platform::MonitorInfo>>) {
    while let Ok(first) = rx.recv() {
        let mut scan = false;
        let mut pending: Vec<(usize, i32)> = Vec::new();
        let mut take = |c: Cmd| match c {
            Cmd::Scan => scan = true,
            Cmd::Set(i, v) => match pending.iter_mut().find(|p| p.0 == i) {
                Some(p) => p.1 = v,
                None => pending.push((i, v)),
            },
        };
        take(first);
        while let Ok(c) = rx.try_recv() {
            take(c);
        }
        if scan {
            let _ = res.send(platform::scan_monitors());
        }
        for (i, v) in pending {
            platform::set_brightness(i, v);
        }
        std::thread::sleep(Duration::from_millis(60));
    }
}

// ---------------------------------------------------------------- ui sync

fn fmt_time(m: i32) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

fn color_u8(c: [f32; 3]) -> slint::Color {
    let b = c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    slint::Color::from_rgb_u8(b[0], b[1], b[2])
}

fn status_text(s: &Settings) -> String {
    if !s.enabled {
        "Desligado".into()
    } else if !s.active() {
        "Agendado · fora do horário".into()
    } else if s.color_mode {
        format!("Cor livre · brilho {}%", s.bright)
    } else {
        format!("{} K · brilho {}%", s.temp, s.bright)
    }
}

fn make_icon(s: &Settings) -> slint::Image {
    // The logo's moon (a disc with a bite taken out), tinted with the current colour.
    const N: u32 = 64;
    let on = s.active();
    let c = s.tint_color().map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
    let (r, g, b, base) = if on { (c[0], c[1], c[2], 1.0) } else { (0x8b, 0x92, 0xa1, 0.55) };
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(N, N);
    let px = buf.make_mut_slice();
    let half = N as f32 / 2.0;
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5 - half, y as f32 + 0.5 - half);
            let disc = (27.0 - (fx * fx + fy * fy).sqrt()).clamp(0.0, 1.0);
            let (bx, by) = (fx - 15.0, fy + 11.5);
            let bite = (21.0 - (bx * bx + by * by).sqrt()).clamp(0.0, 1.0);
            let a = disc * (1.0 - bite) * base;
            px[(y * N + x) as usize] = Rgba8Pixel { r, g, b, a: (a * 255.0) as u8 };
        }
    }
    slint::Image::from_rgba8(buf)
}

fn sync_ui(c: &Ctx) {
    let Some(p) = c.panel.upgrade() else { return };
    let st = c.st.borrow();
    let s = st.s;
    p.set_enabled(s.enabled);
    p.set_color_mode(s.color_mode);
    p.set_temp(s.temp as f32);
    p.set_hue(s.hue as f32);
    p.set_sat(s.sat as f32);
    p.set_strength(s.strength as f32);
    p.set_bright(s.bright as f32);
    p.set_accent(if s.active() { color_u8(s.tint_color()) } else { slint::Color::from_rgb_u8(0x8b, 0x92, 0xa1) });
    p.set_hue_color(color_u8(hsv_to_rgb(s.hue as f32, 1.0, 1.0)));
    p.set_status(status_text(&s).into());
    p.set_sched(s.sched);
    p.set_t1_text(fmt_time(s.t1).into());
    p.set_t2_text(fmt_time(s.t2).into());
    p.set_autostart(platform::autostart_get());
    p.set_link(s.link);
    p.set_master(st.master);
    p.set_scanning(st.scanning);
    let rows: Vec<MonitorRow> = st
        .names
        .iter()
        .zip(&st.mon)
        .map(|(n, v)| MonitorRow { name: n.as_str().into(), value: *v })
        .collect();
    p.set_monitors(ModelRc::new(VecModel::from(rows)));
}

fn sync_tray(c: &Ctx) {
    let Some(t) = c.tray.upgrade() else { return };
    let s = c.st.borrow().s;
    t.set_tray_icon(make_icon(&s));
    t.set_tip(format!("ScreenTint — {}", status_text(&s)).into());
    t.set_toggle_title(if s.enabled { "Desativar" } else { "Ativar" }.into());
}

/// Call after any change to the settings.
fn changed(c: &Rc<Ctx>) {
    sync_ui(c);
    sync_tray(c);
    start_anim(c);
    let c2 = c.clone();
    c.save.start(TimerMode::SingleShot, Duration::from_millis(800), move || {
        model::save(&c2.st.borrow().s);
    });
}

fn start_anim(c: &Rc<Ctx>) {
    if c.anim.running() {
        return;
    }
    let c2 = c.clone();
    c.anim.start(TimerMode::Repeated, Duration::from_millis(16), move || anim_tick(&c2));
}

fn anim_tick(c: &Rc<Ctx>) {
    let mut st = c.st.borrow_mut();
    let t = st.s.target();
    let mut cur = st.cur;
    let mut done = true;
    for i in 0..3 {
        let d = t[i] - cur[i];
        if d.abs() < 0.004 {
            cur[i] = t[i];
        } else {
            cur[i] += d * 0.18;
            done = false;
        }
    }
    st.cur = cur;
    drop(st);
    platform::set_tint(cur);
    if done {
        c.anim.stop();
    }
}

fn update(c: &Rc<Ctx>, f: impl FnOnce(&mut Settings)) {
    f(&mut c.st.borrow_mut().s);
    changed(c);
}

// ---------------------------------------------------------------- panel

fn show_panel(c: &Rc<Ctx>) {
    let Some(p) = c.panel.upgrade() else { return };
    {
        let mut st = c.st.borrow_mut();
        if st.mon.is_empty() && !st.scanning {
            st.scanning = true;
            let _ = c.tx.send(Cmd::Scan);
        }
    }
    sync_ui(c);
    c.st.borrow_mut().placed = (0, 0);
    place_panel(&p);
    let _ = p.show();
    // the real size is only known once the window exists; `poll` re-anchors when it changes
}

/// Keeps the panel's bottom/right edge next to the tray.
fn place_panel(p: &Panel) {
    let size = p.window().size();
    let (x, y) = platform::anchor(size.width as i32, size.height as i32, p.window().scale_factor());
    p.window().set_position(PhysicalPosition::new(x, y));
}

fn toggle_panel(c: &Rc<Ctx>) {
    let Some(p) = c.panel.upgrade() else { return };
    if p.window().is_visible() {
        let _ = p.hide();
        return;
    }
    let recently_hidden = c.st.borrow().last_hide.is_some_and(|t| t.elapsed() < Duration::from_millis(250));
    if !recently_hidden {
        show_panel(c);
    }
}

fn apply_preset(c: &Rc<Ctx>, i: i32) {
    update(c, |s| {
        s.enabled = true;
        s.color_mode = false;
        match i {
            0 => (s.temp, s.strength, s.bright) = (3600, 100, 100),
            1 => (s.temp, s.strength, s.bright) = (2300, 100, 75),
            _ => (s.temp, s.strength, s.bright) = (TEMP_MAX, 0, 40),
        }
    });
}

fn nudge_brightness(c: &Rc<Ctx>, delta: i32) {
    update(c, |s| {
        s.enabled = true;
        s.bright = (s.bright + delta).clamp(BRI_MIN, 100);
    });
}

// ---------------------------------------------------------------- hardware brightness

fn recompute_master(st: &mut State) {
    if !st.mon.is_empty() {
        st.master = st.mon.iter().sum::<f32>() / st.mon.len() as f32;
    }
    st.offsets = st.mon.iter().map(|v| v - st.master).collect();
}

fn master_edited(c: &Rc<Ctx>, v: f32) {
    {
        let mut st = c.st.borrow_mut();
        st.master = v;
        for i in 0..st.mon.len() {
            let nv = (v + st.offsets[i]).clamp(0.0, 100.0);
            st.mon[i] = nv;
            let _ = c.tx.send(Cmd::Set(i, nv.round() as i32));
        }
    }
    sync_ui(c);
}

fn monitor_edited(c: &Rc<Ctx>, i: usize, v: f32) {
    {
        let mut st = c.st.borrow_mut();
        if i < st.mon.len() {
            st.mon[i] = v;
            let _ = c.tx.send(Cmd::Set(i, v.round() as i32));
            recompute_master(&mut st);
        }
    }
    sync_ui(c);
}

fn poll(c: &Rc<Ctx>, scans: &Receiver<Vec<platform::MonitorInfo>>, keys: &[HotKey; 3]) {
    while let Ok(list) = scans.try_recv() {
        {
            let mut st = c.st.borrow_mut();
            st.names = list.iter().map(|m| m.name.clone()).collect();
            st.mon = list.iter().map(|m| m.percent as f32).collect();
            st.scanning = false;
            recompute_master(&mut st);
        }
        sync_ui(c);
    }
    if let Some(p) = c.panel.upgrade() {
        if p.window().is_visible() {
            let sz = p.window().size();
            let changed = {
                let mut st = c.st.borrow_mut();
                let ch = st.placed != (sz.width, sz.height);
                st.placed = (sz.width, sz.height);
                ch
            };
            if changed {
                place_panel(&p);
            }
        }
    }
    if SHOW_REQ.swap(false, Ordering::SeqCst) {
        show_panel(c);
    }
    while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
        if ev.state() != HotKeyState::Pressed {
            continue;
        }
        if ev.id() == keys[0].id() {
            update(c, |s| s.enabled = !s.enabled);
        } else if ev.id() == keys[1].id() {
            nudge_brightness(c, 5);
        } else if ev.id() == keys[2].id() {
            nudge_brightness(c, -5);
        }
    }
}

// ---------------------------------------------------------------- main

fn main() -> Result<(), slint::PlatformError> {
    // Single instance: a second launch just asks the first one to open its panel.
    match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(l) => {
            std::thread::spawn(move || {
                for conn in l.incoming().flatten() {
                    drop(conn);
                    SHOW_REQ.store(true, Ordering::SeqCst);
                }
            });
        }
        Err(_) => {
            let _ = TcpStream::connect(("127.0.0.1", PORT));
            return Ok(());
        }
    }

    platform::init();
    let settings = model::load();
    let panel = Panel::new()?;
    let tray = Tray::new()?;

    let (tx, rx) = channel::<Cmd>();
    let (res_tx, res_rx) = channel();
    std::thread::spawn(move || worker(rx, res_tx));
    let _ = tx.send(Cmd::Scan);

    let ctx = Rc::new(Ctx {
        panel: panel.as_weak(),
        tray: tray.as_weak(),
        st: RefCell::new(State {
            s: settings,
            names: vec![],
            mon: vec![],
            offsets: vec![],
            master: 100.0,
            scanning: true,
            cur: [1.0; 3],
            last_hide: None,
            placed: (0, 0),
        }),
        tx,
        anim: Timer::default(),
        save: Timer::default(),
    });

    // panel callbacks
    {
        let c = ctx.clone();
        panel.on_toggle_enabled(move || update(&c, |s| s.enabled = !s.enabled));
        let c = ctx.clone();
        panel.on_preset(move |i| apply_preset(&c, i));
        let c = ctx.clone();
        panel.on_set_mode(move |color| {
            update(&c, |s| {
                s.color_mode = color;
                s.enabled = true;
                if s.strength < 30 {
                    s.strength = 100;
                }
            })
        });
        let c = ctx.clone();
        panel.on_temp_edited(move |v| {
            update(&c, |s| {
                s.temp = v.round() as i32;
                s.color_mode = false;
                s.enabled = true;
                if s.strength == 0 {
                    s.strength = 100;
                }
            })
        });
        let c = ctx.clone();
        panel.on_hue_edited(move |v| {
            update(&c, |s| {
                s.hue = v.round() as i32;
                s.enabled = true;
            })
        });
        let c = ctx.clone();
        panel.on_sat_edited(move |v| {
            update(&c, |s| {
                s.sat = v.round() as i32;
                s.enabled = true;
            })
        });
        let c = ctx.clone();
        panel.on_strength_edited(move |v| {
            update(&c, |s| {
                s.strength = v.round() as i32;
                s.enabled = true;
            })
        });
        let c = ctx.clone();
        panel.on_bright_edited(move |v| {
            update(&c, |s| {
                s.bright = (v.round() as i32).max(BRI_MIN);
                s.enabled = true;
            })
        });
        let c = ctx.clone();
        panel.on_sched_toggled(move || update(&c, |s| s.sched = !s.sched));
        let c = ctx.clone();
        panel.on_step_time(move |which, d| {
            update(&c, |s| {
                let t = if which == 0 { &mut s.t1 } else { &mut s.t2 };
                *t = (*t + d).rem_euclid(1440);
            })
        });
        let c = ctx.clone();
        panel.on_autostart_toggled(move || {
            platform::autostart_set(!platform::autostart_get());
            changed(&c);
        });
        let c = ctx.clone();
        panel.on_link_toggled(move || {
            {
                let mut st = c.st.borrow_mut();
                st.s.link = !st.s.link;
                if st.s.link {
                    recompute_master(&mut st);
                }
            }
            changed(&c);
        });
        let c = ctx.clone();
        panel.on_master_edited(move |v| master_edited(&c, v));
        let c = ctx.clone();
        panel.on_monitor_edited(move |i, v| monitor_edited(&c, i as usize, v));
    }

    // hide the panel when it loses focus
    {
        let c = ctx.clone();
        panel.window().on_winit_window_event(move |_, ev| {
            if let winit::event::WindowEvent::Focused(false) = ev {
                if let Some(p) = c.panel.upgrade() {
                    if p.window().is_visible() {
                        c.st.borrow_mut().last_hide = Some(Instant::now());
                        let _ = p.hide();
                    }
                }
            }
            EventResult::Propagate
        });
    }

    // tray callbacks
    {
        let c = ctx.clone();
        tray.on_activated(move || toggle_panel(&c));
        let c = ctx.clone();
        tray.on_toggle(move || update(&c, |s| s.enabled = !s.enabled));
        let c = ctx.clone();
        tray.on_preset(move |i| apply_preset(&c, i));
        let c = ctx.clone();
        tray.on_open(move || show_panel(&c));
        tray.on_quit(|| {
            let _ = slint::quit_event_loop();
        });
    }

    // global hotkeys
    let mods = Modifiers::CONTROL | Modifiers::ALT;
    let keys = [
        HotKey::new(Some(mods), Code::KeyT),
        HotKey::new(Some(mods), Code::ArrowUp),
        HotKey::new(Some(mods), Code::ArrowDown),
    ];
    let _hotkeys = GlobalHotKeyManager::new().ok().map(|m| {
        for k in keys {
            let _ = m.register(k);
        }
        m
    });

    // housekeeping timers
    let poll_timer = Timer::default();
    {
        let c = ctx.clone();
        poll_timer.start(TimerMode::Repeated, Duration::from_millis(50), move || poll(&c, &res_rx, &keys));
    }
    let sched_timer = Timer::default();
    {
        let c = ctx.clone();
        sched_timer.start(TimerMode::Repeated, Duration::from_secs(30), move || {
            // schedule boundary crossed, or the effect got lost (display change, UAC, sleep...)
            let (t, cur) = {
                let st = c.st.borrow();
                (st.s.target(), st.cur)
            };
            if t != cur {
                start_anim(&c);
            } else {
                platform::set_tint(t);
            }
            sync_tray(&c);
            sync_ui(&c);
        });
    }

    sync_ui(&ctx);
    changed(&ctx);
    tray.show()?;
    slint::run_event_loop_until_quit()?;

    model::save(&ctx.st.borrow().s);
    platform::shutdown();
    Ok(())
}
