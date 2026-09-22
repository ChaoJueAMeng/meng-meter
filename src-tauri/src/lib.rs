use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::{Networks, System};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::window::Color;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, State, WebviewWindow,
};

mod taskbar;

const EXP_W: f64 = 196.0;
const EXP_H: f64 = 56.0;
const COL_W: f64 = 10.0;
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
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Stats {
    mem_percent: f64,
    up_bps: f64,
    down_bps: f64,
    cpu_c: Option<f64>,
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
                    let items = std::slice::from_raw_parts(
                        raw.as_ptr() as *const Item,
                        count as usize,
                    );
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

    let full_w = (EXP_W * scale).round() as i32;
    let full_h = (EXP_H * scale).round() as i32;
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

    let _ = win.set_size(LogicalSize::new(EXP_W, EXP_H));
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
    let mut sys = state.sys.lock().unwrap();
    sys.refresh_memory();
    let total = sys.total_memory() as f64;
    let avail = sys.available_memory() as f64;
    let mem_percent = if total > 0.0 {
        ((total - avail) / total) * 100.0
    } else {
        0.0
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

    Stats {
        mem_percent,
        up_bps: up as f64 / dt,
        down_bps: down as f64 / dt,
        cpu_c,
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

fn apply_hover(app: &AppHandle, state: &MeterState, hover: bool) {
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
                if st.drag.lock().ok().and_then(|g| *g).is_some() {
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

#[tauri::command]
fn set_hover(app: AppHandle, state: State<MeterState>, hover: bool) {
    apply_hover(&app, &state, hover);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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
        })
        .invoke_handler(tauri::generate_handler![
            get_stats, get_cpu_temp, get_mode, begin_drag, set_hover
        ])
        .setup(|app| {
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "显示", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            if let Some(icon) = app.default_window_icon().cloned() {
                let _ = TrayIconBuilder::new()
                    .icon(icon)
                    .menu(&menu)
                    .show_menu_on_left_click(true)
                    .tooltip("Meng meter")
                    .on_menu_event(|app, event| match event.id.as_ref() {
                        "quit" => app.exit(0),
                        "show" => {
                            if let Some(win) = main_window(app) {
                                let _ = win.show();
                                let _ = win.unminimize();
                                let _ = win.set_always_on_top(true);
                            }
                        }
                        _ => {}
                    })
                    .build(app);
            }

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

            if let Some(win) = main_window(app.handle()) {
                let _ = win.set_background_color(Some(Color(0, 0, 0, 0)));
                apply_mode(app.handle(), &state, true);
                let _ = win.show();
                let _ = win.set_always_on_top(true);
            }

            spawn_hover_watcher(app.handle().clone());
            taskbar::spawn(app.handle().clone());
            let app2 = app.handle().clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(1600));
                let app3 = app2.clone();
                on_main(&app2, move || {
                    let st = app3.state::<MeterState>();
                    if st.hover.lock().ok().map(|g| *g).unwrap_or(false) {
                        return;
                    }
                    if st.dock.lock().ok().and_then(|g| *g).is_none() {
                        return;
                    }
                    apply_mode(&app3, &st, false);
                });
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
