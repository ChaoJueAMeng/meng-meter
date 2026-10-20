use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::{Networks, System};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::window::Color;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, State,
    WebviewWindow, Wry,
};

mod fps;
mod gpu;
mod instance;
mod overlay;
mod settings;
mod taskbar;
mod trayicon;

const NO_FPS_W: f64 = 198.0;
const FPS_W: f64 = 227.0;
const EXP_H: f64 = 67.0;
const COL_W: f64 = 12.0;
const SNAP_PX: i32 = 16;
const ANIM_MS: u64 = 240;
const ANIM_STEP_MS: u64 = 12;

#[derive(Clone, Copy)]
enum Dock {
    Left,
    Right,
}

#[derive(Clone, Copy)]
struct Drag {
    offset_x: i32,
    offset_y: i32,
}

struct FpsMenus {
    ball: MenuItem<Wry>,
    tray_lock: MenuItem<Wry>,
    ball_lock: MenuItem<Wry>,
    popup: Menu<Wry>,
}

struct MeterState {
    sys: Mutex<System>,
    networks: Mutex<Networks>,
    last_net_at: Mutex<Instant>,
    cpu: Mutex<Option<cputemp::Probe>>,
    drag: Mutex<Option<Drag>>,
    dock: Mutex<Option<Dock>>,
    expanded: Mutex<bool>,
    hover: Mutex<bool>,
    anim_id: Mutex<u64>,
    menu_open: Mutex<bool>,
    settings: Mutex<settings::Settings>,
    fps: Mutex<fps::Meter>,
    menus: Mutex<Option<FpsMenus>>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Stats {
    mem_percent: f64,
    up_bps: f64,
    down_bps: f64,
    cpu_c: Option<f64>,
    gpu_c: Option<f64>,
    fps: Option<f64>,
    refresh_hz: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Layout {
    x: i32,
    y: i32,
    dock: Option<String>,
}

#[cfg(windows)]
mod wininput {
    #[link(name = "user32")]
    extern "system" {
        fn GetAsyncKeyState(v_key: i32) -> i16;
    }

    pub fn left_down() -> bool {
        unsafe { GetAsyncKeyState(0x01) < 0 }
    }
}

mod cputemp {
    #[cfg(windows)]
    mod platform {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        #[link(name = "pdh")]
        extern "system" {
            fn PdhOpenQueryW(data_source: *const u16, user_data: usize, query: *mut isize) -> u32;
            fn PdhAddEnglishCounterW(
                query: isize,
                path: *const u16,
                user_data: usize,
                counter: *mut isize,
            ) -> u32;
            fn PdhCollectQueryData(query: isize) -> u32;
            fn PdhGetFormattedCounterArrayW(
                counter: isize,
                format: u32,
                buffer_size: *mut u32,
                item_count: *mut u32,
                item_buffer: *mut u8,
            ) -> u32;
            fn PdhCloseQuery(query: isize) -> u32;
        }

        const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
        const PDH_MORE_DATA: u32 = 0x8000_07D2;
        const PDH_CSTATUS_VALID_DATA: u32 = 0;
        const PDH_CSTATUS_NEW_DATA: u32 = 1;

        #[repr(C)]
        struct FmtValue {
            status: u32,
            _pad: u32,
            value: f64,
        }

        #[repr(C)]
        struct Item {
            name: *const u16,
            value: FmtValue,
        }

        pub struct Probe {
            query: isize,
            counter: isize,
        }

        impl Drop for Probe {
            fn drop(&mut self) {
                unsafe {
                    PdhCloseQuery(self.query);
                }
            }
        }

        fn wide(text: &str) -> Vec<u16> {
            text.encode_utf16().chain(std::iter::once(0)).collect()
        }

        fn read_wide(ptr: *const u16) -> String {
            if ptr.is_null() {
                return String::new();
            }
            let mut len = 0usize;
            unsafe {
                while *ptr.add(len) != 0 && len < 256 {
                    len += 1;
                }
                OsString::from_wide(std::slice::from_raw_parts(ptr, len))
                    .to_string_lossy()
                    .into_owned()
            }
        }

        fn to_celsius(raw: f64) -> Option<f64> {
            let deci_kelvin = raw / 10.0 - 273.15;
            if (5.0..=125.0).contains(&deci_kelvin) {
                return Some(deci_kelvin);
            }
            let kelvin = raw - 273.15;
            if (5.0..=125.0).contains(&kelvin) {
                return Some(kelvin);
            }
            None
        }

        fn rank_zone(name: &str) -> u8 {
            let name = name.to_ascii_lowercase();
            if name.contains("package")
                || name.contains("cpu")
                || name.contains("core")
                || name.contains("tctl")
                || name.contains("tdie")
            {
                2
            } else {
                1
            }
        }

        impl Probe {
            pub fn open() -> Option<Self> {
                const PATHS: [&str; 2] = [
                    r"\Thermal Zone Information(*)\High Precision Temperature",
                    r"\Thermal Zone Information(*)\Temperature",
                ];
                unsafe {
                    let mut query = 0isize;
                    if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                        return None;
                    }
                    for path in PATHS {
                        let path = wide(path);
                        let mut counter = 0isize;
                        if PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) == 0 {
                            let _ = PdhCollectQueryData(query);
                            return Some(Self { query, counter });
                        }
                    }
                    PdhCloseQuery(query);
                    None
                }
            }

            pub fn celsius(&self) -> Option<f64> {
                unsafe {
                    if PdhCollectQueryData(self.query) != 0 {
                        return None;
                    }
                    let mut size = 0u32;
                    let mut count = 0u32;
                    let first = PdhGetFormattedCounterArrayW(
                        self.counter,
                        PDH_FMT_DOUBLE,
                        &mut size,
                        &mut count,
                        std::ptr::null_mut(),
                    );
                    if size == 0 || (first != 0 && first != PDH_MORE_DATA) {
                        return None;
                    }
                    let words = (size as usize).div_ceil(8);
                    let mut raw = vec![0u64; words.max(1)];
                    let status = PdhGetFormattedCounterArrayW(
                        self.counter,
                        PDH_FMT_DOUBLE,
                        &mut size,
                        &mut count,
                        raw.as_mut_ptr() as *mut u8,
                    );
                    if status != 0 || count == 0 {
                        return None;
                    }
                    let items =
                        std::slice::from_raw_parts(raw.as_ptr() as *const Item, count as usize);
                    let mut best: Option<(u8, f64)> = None;
                    for item in items {
                        if item.value.status != PDH_CSTATUS_VALID_DATA
                            && item.value.status != PDH_CSTATUS_NEW_DATA
                        {
                            continue;
                        }
                        let Some(celsius) = to_celsius(item.value.value) else {
                            continue;
                        };
                        let rank = rank_zone(&read_wide(item.name));
                        let replace = match best {
                            Some((prev_rank, prev_temp)) => {
                                rank > prev_rank || (rank == prev_rank && celsius > prev_temp)
                            }
                            None => true,
                        };
                        if replace {
                            best = Some((rank, celsius));
                        }
                    }
                    best.map(|(_, celsius)| celsius)
                }
            }
        }
    }

    #[cfg(not(windows))]
    mod platform {
        pub struct Probe;

        impl Probe {
            pub fn open() -> Option<Self> {
                None
            }

            pub fn celsius(&self) -> Option<f64> {
                None
            }
        }
    }

    pub use platform::Probe;
}

fn memory_percent(sys: &mut System) -> f64 {
    sys.refresh_memory();
    let total = sys.total_memory() as f64;
    let avail = sys.available_memory() as f64;
    if total > 0.0 {
        ((total - avail) / total) * 100.0
    } else {
        0.0
    }
}

fn skip_iface(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("loopback")
        || n.contains("teredo")
        || n.contains("isatap")
        || n.contains("bluetooth")
        || n == "lo"
}

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

fn on_main(app: &AppHandle, f: impl FnOnce() + Send + 'static) {
    let _ = app.run_on_main_thread(f);
}

fn layout_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    Some(dir.join("layout.json"))
}

fn save_layout(app: &AppHandle, state: &MeterState) {
    let Some(win) = main_window(app) else {
        return;
    };
    let Ok(pos) = win.outer_position() else {
        return;
    };
    let dock = state.dock.lock().ok().and_then(|g| {
        (*g).map(|d| match d {
            Dock::Left => "left".to_string(),
            Dock::Right => "right".to_string(),
        })
    });
    let layout = Layout {
        x: pos.x,
        y: pos.y,
        dock,
    };
    if let Some(path) = layout_path(app) {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec(&layout) {
            let _ = fs::write(path, bytes);
        }
    }
}

fn load_layout(app: &AppHandle) -> Option<Layout> {
    let path = layout_path(app)?;
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn show_fps(state: &MeterState) -> bool {
    state
        .settings
        .lock()
        .ok()
        .map(|settings| settings.show_fps)
        .unwrap_or(true)
}

fn expanded_size(state: &MeterState) -> (f64, f64) {
    (if show_fps(state) { FPS_W } else { NO_FPS_W }, EXP_H)
}

fn fps_label(show: bool) -> &'static str {
    if show {
        "关闭FPS显示"
    } else {
        "开启FPS显示"
    }
}

fn sync_fps_menus(state: &MeterState) {
    let label = fps_label(show_fps(state));
    let Ok(menus) = state.menus.lock() else {
        return;
    };
    let Some(menus) = menus.as_ref() else {
        return;
    };
    let _ = menus.ball.set_text(label);
}

fn overlay_locked(state: &MeterState) -> bool {
    state
        .settings
        .lock()
        .ok()
        .map(|settings| settings.overlay_locked)
        .unwrap_or(true)
}

fn lock_label(locked: bool) -> &'static str {
    if locked {
        "解锁悬浮组件"
    } else {
        "锁定悬浮组件"
    }
}

fn sync_lock_menus(state: &MeterState) {
    let label = lock_label(overlay_locked(state));
    let Ok(menus) = state.menus.lock() else {
        return;
    };
    let Some(menus) = menus.as_ref() else {
        return;
    };
    let _ = menus.tray_lock.set_text(label);
    let _ = menus.ball_lock.set_text(label);
}

fn publish_settings(
    app: &AppHandle,
    state: &MeterState,
    settings: settings::Settings,
) -> Result<(), String> {
    settings::save(app, &settings)?;
    if let Ok(mut guard) = state.settings.lock() {
        *guard = settings.clone();
    }
    settings::apply(app, &settings);
    sync_fps_menus(state);
    sync_lock_menus(state);
    emit_fps_display(app, settings.show_fps);
    let _ = app.emit("overlay-lock", settings.overlay_locked);
    Ok(())
}

fn emit_fps_display(app: &AppHandle, show: bool) {
    if let Some(win) = main_window(app) {
        let mode = if show { "on" } else { "off" };
        let _ = win.eval(format!("document.documentElement.dataset.fps='{mode}'"));
    }
    let _ = app.emit("fps-display", show);
}

fn toggle_show_fps(app: &AppHandle, state: &MeterState) {
    let mut next = state
        .settings
        .lock()
        .ok()
        .map(|settings| settings.clone())
        .unwrap_or_default();
    next.show_fps = !next.show_fps;
    if settings::save(app, &next).is_err() {
        return;
    }
    if let Ok(mut settings) = state.settings.lock() {
        *settings = next.clone();
    }
    sync_fps_menus(state);
    emit_fps_display(app, next.show_fps);
    if next.floating {
        let expanded = state
            .expanded
            .lock()
            .ok()
            .map(|expanded| *expanded)
            .unwrap_or(true);
        apply_mode(app, state, expanded);
    }
}

fn menu_anchor(win: &WebviewWindow) -> Option<LogicalPosition<f64>> {
    let cursor = win.cursor_position().ok()?;
    let origin = win.outer_position().ok()?;
    let size = win.outer_size().ok()?;
    let scale = win.scale_factor().unwrap_or(1.0).max(0.01);
    let width = size.width as f64 / scale;
    let height = size.height as f64 / scale;
    let x = ((cursor.x - origin.x as f64) / scale).clamp(0.0, width);
    Some(LogicalPosition::new(x, height))
}

fn ease_out_cubic(t: f64) -> f64 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

fn lerp_i(a: i32, b: i32, t: f64) -> i32 {
    (a as f64 + (b as f64 - a as f64) * t).round() as i32
}

fn bump_anim(state: &MeterState) -> u64 {
    if let Ok(mut g) = state.anim_id.lock() {
        *g += 1;
        *g
    } else {
        0
    }
}

fn emit_mode(win: &WebviewWindow, expanded: bool, dock: Option<Dock>) {
    let mode = match (expanded, dock) {
        (false, Some(Dock::Left)) => "collapsed-left",
        (false, Some(Dock::Right)) => "collapsed-right",
        _ => "expanded",
    };
    let _ = win.emit("mode", mode);
}

fn apply_mode(app: &AppHandle, state: &MeterState, expanded: bool) {
    if let Ok(mut g) = state.expanded.lock() {
        *g = expanded;
    }
    let Some(win) = main_window(app) else {
        return;
    };
    let scale = win.scale_factor().unwrap_or(1.0);
    let dock = state.dock.lock().ok().and_then(|g| *g);
    let Ok(pos) = win.outer_position() else {
        return;
    };
    let Ok(size) = win.outer_size() else {
        return;
    };

    let (exp_w, exp_h) = expanded_size(state);
    let full_w = (exp_w * scale).round() as i32;
    let full_h = (exp_h * scale).round() as i32;
    let peek = (COL_W * scale).round() as i32;
    let cy = pos.y + size.height as i32 / 2;
    let mut new_y = cy - full_h / 2;

    let monitor = win.current_monitor().ok().flatten();
    if let Some(m) = &monitor {
        let top = m.position().y;
        let bottom = top + m.size().height as i32;
        new_y = new_y.clamp(top, (bottom - full_h).max(top));
    }

    let new_x = match (expanded, dock) {
        (false, Some(Dock::Right)) => monitor
            .as_ref()
            .map(|m| m.position().x + m.size().width as i32 - peek)
            .unwrap_or(pos.x + size.width as i32 - peek),
        (false, Some(Dock::Left)) => monitor
            .as_ref()
            .map(|m| m.position().x - (full_w - peek))
            .unwrap_or(pos.x),
        (true, Some(Dock::Right)) => monitor
            .as_ref()
            .map(|m| m.position().x + m.size().width as i32 - full_w)
            .unwrap_or(pos.x + size.width as i32 - full_w),
        (true, Some(Dock::Left)) => monitor.as_ref().map(|m| m.position().x).unwrap_or(pos.x),
        _ => pos.x,
    };

    let _ = win.set_size(LogicalSize::new(exp_w, exp_h));
    let id = bump_anim(state);
    let from_x = pos.x;
    let from_y = pos.y;
    let animate = dock.is_some() && ((from_x - new_x).abs() > 2 || (from_y - new_y).abs() > 2);

    if !animate {
        if from_x != new_x || from_y != new_y {
            let _ = win.set_position(PhysicalPosition::new(new_x, new_y));
        }
        emit_mode(&win, expanded, dock);
        return;
    }

    if expanded {
        emit_mode(&win, true, dock);
    }

    let app2 = app.clone();
    thread::spawn(move || {
        let frames = (ANIM_MS / ANIM_STEP_MS).max(1);
        for i in 1..=frames {
            thread::sleep(Duration::from_millis(ANIM_STEP_MS));
            let st = app2.state::<MeterState>();
            if st.anim_id.lock().ok().map(|g| *g) != Some(id) {
                return;
            }
            let t = ease_out_cubic(i as f64 / frames as f64);
            let x = lerp_i(from_x, new_x, t);
            let y = lerp_i(from_y, new_y, t);
            let app3 = app2.clone();
            on_main(&app2, move || {
                if let Some(win) = main_window(&app3) {
                    let _ = win.set_position(PhysicalPosition::new(x, y));
                }
            });
        }
        let app3 = app2.clone();
        on_main(&app2, move || {
            let st = app3.state::<MeterState>();
            if st.anim_id.lock().ok().map(|g| *g) != Some(id) {
                return;
            }
            if let Some(win) = main_window(&app3) {
                let _ = win.set_position(PhysicalPosition::new(new_x, new_y));
                emit_mode(&win, expanded, dock);
            }
        });
    });
}

fn snap_if_needed(app: &AppHandle, state: &MeterState) {
    let Some(win) = main_window(app) else {
        return;
    };
    let Ok(pos) = win.outer_position() else {
        return;
    };
    let Ok(size) = win.outer_size() else {
        return;
    };
    let Some(m) = win.current_monitor().ok().flatten() else {
        return;
    };
    let mx = m.position().x;
    let mw = m.size().width as i32;
    let right = pos.x + size.width as i32;
    let mon_right = mx + mw;
    let hovering = state.hover.lock().ok().map(|g| *g).unwrap_or(false);

    if pos.x - mx <= SNAP_PX {
        if let Ok(mut g) = state.dock.lock() {
            *g = Some(Dock::Left);
        }
        apply_mode(app, state, hovering);
    } else if mon_right - right <= SNAP_PX {
        if let Ok(mut g) = state.dock.lock() {
            *g = Some(Dock::Right);
        }
        apply_mode(app, state, hovering);
    } else {
        if let Ok(mut g) = state.dock.lock() {
            *g = None;
        }
        apply_mode(app, state, true);
    }
}

#[tauri::command]
fn get_stats(state: State<MeterState>) -> Stats {
    let mem_percent = {
        let mut sys = state.sys.lock().unwrap();
        memory_percent(&mut sys)
    };

    let mut nets = state.networks.lock().unwrap();
    nets.refresh(true);

    let mut last_at = state.last_net_at.lock().unwrap();
    let dt = last_at.elapsed().as_secs_f64().max(0.05);
    *last_at = Instant::now();

    let mut down = 0u64;
    let mut up = 0u64;
    for (name, data) in nets.iter() {
        if skip_iface(name) {
            continue;
        }
        down += data.received();
        up += data.transmitted();
    }

    let cpu_c = {
        let mut cpu = state.cpu.lock().unwrap();
        if cpu.is_none() {
            *cpu = cputemp::Probe::open();
        }
        cpu.as_ref().and_then(|probe| probe.celsius())
    };

    let (fps, refresh_hz) = state
        .fps
        .lock()
        .ok()
        .map(|meter| (meter.frames_per_sec(), meter.refresh_hz()))
        .unwrap_or((None, None));

    Stats {
        mem_percent,
        up_bps: up as f64 / dt,
        down_bps: down as f64 / dt,
        cpu_c,
        gpu_c: gpu::selected_celsius(),
        fps,
        refresh_hz,
    }
}

#[tauri::command]
fn get_cpu_temp(state: State<MeterState>) -> Option<f64> {
    let mut cpu = state.cpu.lock().unwrap();
    if cpu.is_none() {
        *cpu = cputemp::Probe::open();
    }
    cpu.as_ref().and_then(|probe| probe.celsius())
}

#[tauri::command]
fn get_mode(state: State<MeterState>) -> String {
    let expanded = state.expanded.lock().ok().map(|g| *g).unwrap_or(true);
    let dock = state.dock.lock().ok().and_then(|g| *g);
    match (expanded, dock) {
        (false, Some(Dock::Left)) => "collapsed-left".into(),
        (false, Some(Dock::Right)) => "collapsed-right".into(),
        _ => "expanded".into(),
    }
}

fn cursor_inside(win: &WebviewWindow, state: &MeterState) -> bool {
    let Ok(pos) = win.outer_position() else {
        return false;
    };
    let Ok(size) = win.outer_size() else {
        return false;
    };
    let Ok(cursor) = win.cursor_position() else {
        return false;
    };
    let scale = win.scale_factor().unwrap_or(1.0);
    let peek = (COL_W * scale).round() as i32;
    let pad = 8;
    let expanded = state.expanded.lock().ok().map(|g| *g).unwrap_or(true);
    let dock = state.dock.lock().ok().and_then(|g| *g);
    let x = cursor.x as i32;
    let y = cursor.y as i32;
    let (x0, x1) = if !expanded {
        match dock {
            Some(Dock::Right) => (pos.x - pad, pos.x + peek + pad),
            Some(Dock::Left) => {
                let right = pos.x + size.width as i32;
                (right - peek - pad, right + pad)
            }
            None => (pos.x - pad, pos.x + size.width as i32 + pad),
        }
    } else {
        (pos.x - pad, pos.x + size.width as i32 + pad)
    };
    x >= x0 && x < x1 && y >= pos.y - pad && y < pos.y + size.height as i32 + pad
}

fn menu_open(state: &MeterState) -> bool {
    state
        .menu_open
        .lock()
        .ok()
        .map(|open| *open)
        .unwrap_or(false)
}

fn apply_hover(app: &AppHandle, state: &MeterState, hover: bool) {
    if menu_open(state) {
        if let Ok(mut g) = state.hover.lock() {
            *g = true;
        }
        return;
    }
    if let Ok(mut g) = state.hover.lock() {
        *g = hover;
    }
    if state.drag.lock().ok().and_then(|g| *g).is_some() {
        return;
    }
    let docked = state.dock.lock().ok().and_then(|g| *g).is_some();
    if !docked {
        apply_mode(app, state, true);
        return;
    }
    if hover {
        apply_mode(app, state, true);
        return;
    }

    let app2 = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(400));
        let app3 = app2.clone();
        on_main(&app2, move || {
            let st = app3.state::<MeterState>();
            if st.hover.lock().ok().map(|g| *g).unwrap_or(true) {
                return;
            }
            if menu_open(&st) {
                return;
            }
            if st.drag.lock().ok().and_then(|g| *g).is_some() {
                return;
            }
            if st.dock.lock().ok().and_then(|g| *g).is_none() {
                return;
            }
            apply_mode(&app3, &st, false);
            save_layout(&app3, &st);
        });
    });
}

fn spawn_hover_watcher(app: AppHandle) {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(800));
        loop {
            thread::sleep(Duration::from_millis(80));
            let app2 = app.clone();
            on_main(&app, move || {
                let Some(win) = main_window(&app2) else {
                    return;
                };
                let st = app2.state::<MeterState>();
                if st.drag.lock().ok().and_then(|g| *g).is_some() || menu_open(&st) {
                    return;
                }
                let inside = cursor_inside(&win, &st);
                let prev = st.hover.lock().ok().map(|g| *g).unwrap_or(false);
                if inside != prev {
                    apply_hover(&app2, &st, inside);
                }
            });
        }
    });
}

#[tauri::command]
fn begin_drag(app: AppHandle, state: State<MeterState>) {
    if let Ok(mut g) = state.dock.lock() {
        *g = None;
    }
    apply_mode(&app, &state, true);

    let Some(win) = main_window(&app) else {
        return;
    };
    let Ok(cursor) = win.cursor_position() else {
        return;
    };
    let Ok(pos) = win.outer_position() else {
        return;
    };
    if let Ok(mut g) = state.drag.lock() {
        *g = Some(Drag {
            offset_x: cursor.x as i32 - pos.x,
            offset_y: cursor.y as i32 - pos.y,
        });
    }

    let app2 = app.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(8));
            #[cfg(windows)]
            if !wininput::left_down() {
                break;
            }
            #[cfg(not(windows))]
            break;

            let app3 = app2.clone();
            on_main(&app2, move || {
                let Some(win) = main_window(&app3) else {
                    return;
                };
                let st = app3.state::<MeterState>();
                let drag = match st.drag.lock().ok().and_then(|g| *g) {
                    Some(d) => d,
                    None => return,
                };
                if let Ok(cursor) = win.cursor_position() {
                    let x = cursor.x as i32 - drag.offset_x;
                    let y = cursor.y as i32 - drag.offset_y;
                    let _ = win.set_position(PhysicalPosition::new(x, y));
                }
            });
        }

        let app3 = app2.clone();
        on_main(&app2, move || {
            let st = app3.state::<MeterState>();
            if let Ok(mut g) = st.drag.lock() {
                *g = None;
            }
            snap_if_needed(&app3, &st);
            save_layout(&app3, &st);
        });
    });
}

fn tray_tooltip(percent: f64) -> String {
    format!("Meng meter  内存 {:.0}%", percent.clamp(0.0, 100.0))
}

fn spawn_tray_updater(app: AppHandle) {
    thread::spawn(move || {
        let mut history = VecDeque::with_capacity(trayicon::SAMPLES);
        loop {
            let percent = app
                .state::<MeterState>()
                .sys
                .lock()
                .ok()
                .map(|mut sys| memory_percent(&mut sys))
                .unwrap_or(0.0);
            if history.len() == trayicon::SAMPLES {
                history.pop_front();
            }
            history.push_back(percent);
            let samples: Vec<f64> = history.iter().copied().collect();
            if let Some(tray) = app.tray_by_id(trayicon::ID) {
                let _ = tray.set_icon(Some(trayicon::render(&samples)));
                let _ = tray.set_tooltip(Some(tray_tooltip(percent)));
            }
            thread::sleep(Duration::from_millis(1000));
        }
    });
}

#[tauri::command]
fn open_ball_menu(app: AppHandle) {
    thread::spawn(move || {
        let state = app.state::<MeterState>();
        let popup = {
            let menus = state.menus.lock().ok();
            menus.and_then(|menus| menus.as_ref().map(|menus| menus.popup.clone()))
        };
        let Some(popup) = popup else {
            return;
        };
        let Some(win) = main_window(&app) else {
            return;
        };
        if let Ok(mut hover) = state.hover.lock() {
            *hover = true;
        }
        if let Ok(mut open) = state.menu_open.lock() {
            *open = true;
        }
        let _ = win.set_always_on_top(false);
        let opened = match menu_anchor(&win) {
            Some(anchor) => win.popup_menu_at(&popup, anchor),
            None => win.popup_menu(&popup),
        };
        let _ = opened;
        if let Ok(mut open) = state.menu_open.lock() {
            *open = false;
        }
        let floating = state
            .settings
            .lock()
            .ok()
            .map(|settings| settings.floating)
            .unwrap_or(false);
        if floating {
            let _ = win.set_always_on_top(true);
        }
        let inside = cursor_inside(&win, &state);
        apply_hover(&app, &state, inside);
    });
}

#[tauri::command]
fn set_hover(app: AppHandle, state: State<MeterState>, hover: bool) {
    apply_hover(&app, &state, hover);
}

#[tauri::command]
fn get_settings(state: State<MeterState>) -> settings::Settings {
    state.settings.lock().map(|g| g.clone()).unwrap_or_default()
}

#[tauri::command]
fn save_settings(
    app: AppHandle,
    state: State<MeterState>,
    mut settings: settings::Settings,
) -> Result<settings::Settings, String> {
    settings.configured = true;
    publish_settings(&app, &state, settings.clone())?;
    if settings.floating {
        apply_mode(&app, &state, true);
    }
    Ok(settings)
}

#[tauri::command]
fn close_settings(app: AppHandle, state: State<MeterState>) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.hide();
    }
    let floating = state
        .settings
        .lock()
        .ok()
        .map(|settings| settings.floating)
        .unwrap_or(false);
    if floating {
        apply_mode(&app, &state, true);
        maybe_collapse_later(app.clone());
    }
}

#[tauri::command]
fn list_gpus() -> Vec<gpu::GpuInfo> {
    gpu::list()
}

#[tauri::command]
fn get_overlay(
    app: AppHandle,
    state: State<MeterState>,
    kind: String,
) -> Option<overlay::OverlayView> {
    let (locked, shown) = state
        .settings
        .lock()
        .map(|settings| (settings.overlay_locked, overlay_shown(&settings, &kind)))
        .unwrap_or((true, false));
    overlay::view(&app, &kind, locked, shown)
}

fn overlay_shown(settings: &settings::Settings, kind: &str) -> bool {
    match kind {
        "gpu" => settings.overlay_gpu,
        "fps" => settings.overlay_fps,
        _ => settings.overlay_cpu,
    }
}

#[tauri::command]
fn save_overlay_style(
    app: AppHandle,
    kind: String,
    color: String,
    opacity: f64,
) -> Result<(), String> {
    overlay::save_style(&app, &kind, &color, opacity)
}

#[tauri::command]
fn overlay_begin_drag(app: AppHandle, kind: String) {
    overlay::begin_drag(app, kind);
}

#[tauri::command]
fn overlay_begin_resize(app: AppHandle, kind: String, corner: String) {
    overlay::begin_resize(app, kind, corner);
}

#[tauri::command]
fn set_overlay_locked(
    app: AppHandle,
    state: State<MeterState>,
    locked: bool,
) -> Result<(), String> {
    let mut next = state.settings.lock().map(|g| g.clone()).unwrap_or_default();
    next.overlay_locked = locked;
    settings::save(&app, &next)?;
    if let Ok(mut guard) = state.settings.lock() {
        *guard = next.clone();
    }
    sync_lock_menus(&state);
    overlay::apply(&app, &next);
    let _ = app.emit("overlay-lock", next.overlay_locked);
    Ok(())
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayShown {
    kind: String,
    shown: bool,
}

#[tauri::command]
fn set_overlay_shown(
    app: AppHandle,
    state: State<MeterState>,
    kind: String,
    shown: bool,
) -> Result<(), String> {
    if !matches!(kind.as_str(), "cpu" | "gpu" | "fps") {
        return Err("未知悬浮组件".into());
    }
    let mut next = state.settings.lock().map(|g| g.clone()).unwrap_or_default();
    match kind.as_str() {
        "gpu" => next.overlay_gpu = shown,
        "fps" => next.overlay_fps = shown,
        _ => next.overlay_cpu = shown,
    }
    settings::save(&app, &next)?;
    if let Ok(mut guard) = state.settings.lock() {
        *guard = next.clone();
    }
    overlay::apply(&app, &next);
    let _ = app.emit("overlay-shown", OverlayShown { kind, shown });
    Ok(())
}

fn open_settings(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

fn show_main_if_enabled(app: &AppHandle, state: &MeterState) {
    let settings = state
        .settings
        .lock()
        .ok()
        .map(|g| g.clone())
        .unwrap_or_default();
    if !settings.configured {
        open_settings(app);
        return;
    }
    if !settings.floating {
        if let Some(win) = main_window(app) {
            let _ = win.hide();
        }
        return;
    }
    if let Some(win) = main_window(app) {
        let _ = win.set_background_color(Some(Color(0, 0, 0, 0)));
        apply_mode(app, state, true);
        let _ = win.show();
        let _ = win.set_always_on_top(true);
    }
}

fn maybe_collapse_later(app: AppHandle) {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(1600));
        let app3 = app.clone();
        on_main(&app, move || {
            let st = app3.state::<MeterState>();
            if !st.settings.lock().ok().map(|g| g.floating).unwrap_or(false) {
                return;
            }
            if st.hover.lock().ok().map(|g| *g).unwrap_or(false) {
                return;
            }
            if st.dock.lock().ok().and_then(|g| *g).is_none() {
                return;
            }
            apply_mode(&app3, &st, false);
        });
    });
}

fn bind_settings_window(app: &AppHandle) {
    let Some(win) = app.get_webview_window("settings") else {
        return;
    };
    let handle = app.clone();
    win.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let configured = handle
                .state::<MeterState>()
                .settings
                .lock()
                .ok()
                .map(|g| g.configured)
                .unwrap_or(false);
            if !configured {
                handle.exit(0);
                return;
            }
            if let Some(win) = handle.get_webview_window("settings") {
                let _ = win.hide();
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    instance::acquire();
    let mut sys = System::new();
    sys.refresh_memory();
    let mut networks = Networks::new_with_refreshed_list();
    networks.refresh(true);

    tauri::Builder::default()
        .manage(MeterState {
            sys: Mutex::new(sys),
            networks: Mutex::new(networks),
            last_net_at: Mutex::new(Instant::now()),
            cpu: Mutex::new(None),
            drag: Mutex::new(None),
            dock: Mutex::new(None),
            expanded: Mutex::new(true),
            hover: Mutex::new(false),
            anim_id: Mutex::new(0),
            menu_open: Mutex::new(false),
            settings: Mutex::new(settings::Settings::default()),
            fps: Mutex::new(fps::Meter::start()),
            menus: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            get_stats,
            get_cpu_temp,
            get_mode,
            begin_drag,
            set_hover,
            get_settings,
            save_settings,
            close_settings,
            open_ball_menu,
            list_gpus,
            get_overlay,
            save_overlay_style,
            overlay_begin_drag,
            overlay_begin_resize,
            set_overlay_locked,
            set_overlay_shown
        ])
        .setup(|app| {
            let loaded = settings::load(app.handle());
            if let Ok(mut g) = app.state::<MeterState>().settings.lock() {
                *g = loaded.clone();
            }

            let label = fps_label(loaded.show_fps);
            let lock = lock_label(loaded.overlay_locked);
            let settings_item = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let tray_lock = MenuItem::with_id(app, "overlay-lock", lock, true, None::<&str>)?;
            let ball_settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
            let ball_quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let ball_fps = MenuItem::with_id(app, "fps", label, true, None::<&str>)?;
            let ball_lock = MenuItem::with_id(app, "overlay-lock", lock, true, None::<&str>)?;
            let popup =
                Menu::with_items(app, &[&ball_settings, &ball_fps, &ball_lock, &ball_quit])?;
            if let Ok(mut menus) = app.state::<MeterState>().menus.lock() {
                *menus = Some(FpsMenus {
                    ball: ball_fps,
                    tray_lock: tray_lock.clone(),
                    ball_lock,
                    popup,
                });
            }
            let menu = Menu::with_items(app, &[&settings_item, &tray_lock, &quit])?;
            let percent = app
                .state::<MeterState>()
                .sys
                .lock()
                .ok()
                .map(|mut sys| memory_percent(&mut sys))
                .unwrap_or(0.0);
            let _ = TrayIconBuilder::with_id(trayicon::ID)
                .icon(trayicon::render(&[percent]))
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip(tray_tooltip(percent))
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        open_settings(tray.app_handle());
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "settings" => open_settings(app),
                    "fps" => {
                        let handle = app.clone();
                        thread::spawn(move || {
                            let st = handle.state::<MeterState>();
                            toggle_show_fps(&handle, &st);
                        });
                    }
                    "overlay-lock" => {
                        let handle = app.clone();
                        thread::spawn(move || {
                            let st = handle.state::<MeterState>();
                            let locked = overlay_locked(&st);
                            let _ = set_overlay_locked(handle.clone(), st, !locked);
                        });
                    }
                    _ => {}
                })
                .build(app);
            spawn_tray_updater(app.handle().clone());
            bind_settings_window(app.handle());
            let focus_app = app.handle().clone();
            instance::listen(move || {
                let app2 = focus_app.clone();
                on_main(&focus_app, move || {
                    open_settings(&app2);
                });
            });

            let state = app.state::<MeterState>();
            if let Some(layout) = load_layout(app.handle()) {
                if let Ok(mut g) = state.dock.lock() {
                    *g = match layout.dock.as_deref() {
                        Some("left") => Some(Dock::Left),
                        Some("right") => Some(Dock::Right),
                        _ => None,
                    };
                }
                if let Some(win) = main_window(app.handle()) {
                    let _ = win.set_position(PhysicalPosition::new(layout.x, layout.y));
                }
            }

            spawn_hover_watcher(app.handle().clone());
            gpu::spawn();
            taskbar::spawn(app.handle().clone());

            if loaded.configured {
                emit_fps_display(app.handle(), loaded.show_fps);
                if let Some(win) = main_window(app.handle()) {
                    let (width, height) = expanded_size(&state);
                    let _ = win.set_size(LogicalSize::new(width, height));
                }
                settings::apply(app.handle(), &loaded);
                if loaded.floating {
                    show_main_if_enabled(app.handle(), &state);
                    maybe_collapse_later(app.handle().clone());
                }
            } else {
                open_settings(app.handle());
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
