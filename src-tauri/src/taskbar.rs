use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewWindow};

const LABEL: &str = "taskbar";
const WIDTH_DIP: f64 = 72.0;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Spot {
    tray_left: i32,
    y: i32,
    h: i32,
    dark_text: bool,
}

#[derive(Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Theme {
    fg: String,
    shadow: String,
    compact: bool,
}

struct Applied {
    shown: bool,
    x: i32,
    y: i32,
    h: f64,
    theme: Option<Theme>,
    tuned: bool,
    tick: u32,
}

static APPLIED: Mutex<Applied> = Mutex::new(Applied {
    shown: false,
    x: i32::MIN,
    y: i32::MIN,
    h: -1.0,
    theme: None,
    tuned: false,
    tick: 0,
});

static ENABLED: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
static PREV_PROC: AtomicIsize = AtomicIsize::new(0);
#[cfg(windows)]
static ALLOW_HIDE: AtomicBool = AtomicBool::new(false);

pub fn set_enabled(app: &AppHandle, enabled: bool) {
    ENABLED.store(enabled, Ordering::SeqCst);
    if enabled {
        return;
    }
    let Some(win) = app.get_webview_window(LABEL) else {
        return;
    };
    if let Ok(mut state) = APPLIED.lock() {
        if state.shown {
            conceal(&win);
            state.shown = false;
        }
    }
}

pub fn spawn(app: AppHandle) {
    #[cfg(windows)]
    spawn_win(app);
    #[cfg(not(windows))]
    let _ = app;
}

#[cfg(windows)]
fn spawn_win(app: AppHandle) {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(350));
        loop {
            let spot = measure();
            let app2 = app.clone();
            on_main(&app, move || apply(&app2, spot));
            thread::sleep(Duration::from_millis(120));
        }
    });
}

fn on_main(app: &AppHandle, f: impl FnOnce() + Send + 'static) {
    let _ = app.run_on_main_thread(f);
}

fn apply(app: &AppHandle, spot: Option<Spot>) {
    let Some(win) = app.get_webview_window(LABEL) else {
        return;
    };
    let Ok(mut state) = APPLIED.lock() else {
        return;
    };

    if !ENABLED.load(Ordering::SeqCst) {
        if state.shown {
            conceal(&win);
            state.shown = false;
        }
        return;
    }

    let Some(spot) = spot else {
        if state.shown {
            conceal(&win);
            state.shown = false;
        }
        return;
    };

    if !state.tuned {
        let _ = win.set_focusable(false);
        let _ = win.set_shadow(false);
        let _ = win.set_ignore_cursor_events(true);
        let _ = win.set_always_on_top(true);
        let _ = win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        tune_native(&win);
        state.tuned = true;
    }

    let scale = win.scale_factor().unwrap_or(1.0).max(0.5);
    let logical_h = (spot.h as f64 / scale).max(1.0);
    let compact = logical_h < 42.0;
    let theme = if spot.dark_text {
        Theme {
            fg: "#1c1917".into(),
            shadow: "rgba(255,255,255,0.45)".into(),
            compact,
        }
    } else {
        Theme {
            fg: "#ffffff".into(),
            shadow: "rgba(0,0,0,0.45)".into(),
            compact,
        }
    };

    let phys_w = (WIDTH_DIP * scale).round() as i32;
    let x = spot.tray_left - phys_w;
    let drifted = window_drifted(&win, x, spot.y);
    let moved = state.x != x || state.y != spot.y || (state.h - logical_h).abs() > 0.5 || drifted;
    if moved {
        let _ = win.set_size(LogicalSize::new(WIDTH_DIP, logical_h));
        let _ = win.set_position(PhysicalPosition::new(x, spot.y));
        state.x = x;
        state.y = spot.y;
        state.h = logical_h;
    }

    if !state.shown {
        let _ = win.show();
        let _ = win.set_focusable(false);
        state.shown = true;
    }
    keep_visible(&win);

    state.tick = state.tick.wrapping_add(1);
    let theme_changed = state.theme.as_ref() != Some(&theme);
    if theme_changed || state.tick % 8 == 0 {
        let _ = win.emit("taskbar-theme", &theme);
        state.theme = Some(theme);
    }
}

#[cfg(windows)]
fn window_drifted(win: &WebviewWindow, x: i32, y: i32) -> bool {
    let Some(hwnd) = native_hwnd(win) else {
        return false;
    };
    unsafe {
        let mut rc = Rect::zero();
        if GetWindowRect(hwnd, &mut rc) == 0 {
            return true;
        }
        (rc.left - x).abs() > 2 || (rc.top - y).abs() > 2
    }
}

#[cfg(not(windows))]
fn window_drifted(_win: &WebviewWindow, _x: i32, _y: i32) -> bool {
    false
}

#[cfg(windows)]
fn tune_native(win: &WebviewWindow) {
    let Some(hwnd) = native_hwnd(win) else {
        return;
    };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            ex | WS_EX_NOACTIVATE
                | WS_EX_TOOLWINDOW
                | WS_EX_LAYERED
                | WS_EX_TRANSPARENT
                | WS_EX_TOPMOST,
        );
        let taskbar = FindWindowW(wide("Shell_TrayWnd").as_ptr(), std::ptr::null());
        if taskbar != 0 {
            SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, taskbar);
        }
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
        install_guard(hwnd);
    }
}

#[cfg(windows)]
fn conceal(win: &WebviewWindow) {
    ALLOW_HIDE.store(true, Ordering::SeqCst);
    let _ = win.hide();
    ALLOW_HIDE.store(false, Ordering::SeqCst);
}

#[cfg(not(windows))]
fn conceal(win: &WebviewWindow) {
    let _ = win.hide();
}

#[cfg(windows)]
fn keep_visible(win: &WebviewWindow) {
    let Some(hwnd) = native_hwnd(win) else {
        return;
    };
    unsafe {
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked, 4) == 0 && cloaked != 0 {
            let shown = 0u32;
            DwmSetWindowAttribute(hwnd, DWMWA_CLOAK, &shown, 4);
        }
        if IsIconic(hwnd) != 0 {
            let mut place = WindowPlacement {
                length: std::mem::size_of::<WindowPlacement>() as u32,
                flags: 0,
                show_cmd: SW_SHOWNOACTIVATE,
                min_position: Point { x: 0, y: 0 },
                max_position: Point { x: 0, y: 0 },
                normal_position: Rect::zero(),
            };
            if GetWindowPlacement(hwnd, &mut place) != 0 {
                place.length = std::mem::size_of::<WindowPlacement>() as u32;
                place.show_cmd = SW_SHOWNOACTIVATE;
                SetWindowPlacement(hwnd, &place);
            }
        } else if IsWindowVisible(hwnd) == 0 {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE as i32);
        }
        lift_above_taskbar(hwnd);
    }
}

#[cfg(windows)]
fn lift_above_taskbar(hwnd: isize) {
    unsafe {
        let taskbar = FindWindowW(wide("Shell_TrayWnd").as_ptr(), std::ptr::null());
        if taskbar == 0 || menu_is_open() {
            return;
        }
        let desktop = show_desktop_active();
        if !desktop && !window_is_above(taskbar, hwnd) {
            return;
        }
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
    }
}

#[cfg(windows)]
fn window_is_above(upper: isize, lower: isize) -> bool {
    unsafe {
        let mut hwnd = GetWindow(lower, GW_HWNDPREV);
        let mut steps = 0;
        while hwnd != 0 && steps < 48 {
            if hwnd == upper {
                return true;
            }
            hwnd = GetWindow(hwnd, GW_HWNDPREV);
            steps += 1;
        }
        false
    }
}

#[cfg(windows)]
fn show_desktop_active() -> bool {
    matches!(class_name(unsafe { GetForegroundWindow() }).as_str(), "Progman" | "WorkerW")
}

#[cfg(windows)]
fn menu_is_open() -> bool {
    let class = class_name(unsafe { GetForegroundWindow() });
    class == "#32768" || class == "NotifyIconOverflowWindow"
}

#[cfg(not(windows))]
fn keep_visible(_win: &WebviewWindow) {}

#[cfg(windows)]
fn install_guard(hwnd: isize) {
    if PREV_PROC.load(Ordering::SeqCst) != 0 {
        return;
    }
    let prev = unsafe {
        SetWindowLongPtrW(
            hwnd,
            GWLP_WNDPROC,
            temp_wnd_proc as *const () as usize as isize,
        )
    };
    if prev != 0 {
        PREV_PROC.store(prev, Ordering::SeqCst);
    }
}

#[cfg(windows)]
unsafe extern "system" fn temp_wnd_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    if !ALLOW_HIDE.load(Ordering::SeqCst) {
        match msg {
            WM_WINDOWPOSCHANGING => {
                let pos = &mut *(lparam as *mut WindowPos);
                if pos.flags & SWP_HIDEWINDOW != 0 {
                    pos.flags = (pos.flags & !SWP_HIDEWINDOW) | SWP_SHOWWINDOW;
                }
                if pos.x <= -30_000 || pos.y <= -30_000 {
                    pos.flags |= SWP_NOMOVE | SWP_NOSIZE;
                }
            }
            WM_SHOWWINDOW if wparam == 0 => return 0,
            WM_SYSCOMMAND if wparam & 0xFFF0 == SC_MINIMIZE => return 0,
            _ => {}
        }
    }
    let prev = PREV_PROC.load(Ordering::SeqCst);
    if prev == 0 {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    CallWindowProcW(prev, hwnd, msg, wparam, lparam)
}

#[cfg(not(windows))]
fn tune_native(_win: &WebviewWindow) {}

#[cfg(windows)]
fn native_hwnd(win: &WebviewWindow) -> Option<isize> {
    win.hwnd().ok().map(|hwnd| hwnd.0 as isize)
}

#[cfg(windows)]
fn measure() -> Option<Spot> {
    unsafe {
        let taskbar = FindWindowW(wide("Shell_TrayWnd").as_ptr(), std::ptr::null());
        if taskbar == 0 {
            return None;
        }
        let mut bar = Rect::zero();
        if GetWindowRect(taskbar, &mut bar) == 0 {
            return None;
        }
        let monitor = MonitorFromWindow(taskbar, MONITOR_DEFAULTTONEAREST);
        let mut info = MonitorInfo {
            cb_size: std::mem::size_of::<MonitorInfo>() as u32,
            rc_monitor: Rect::zero(),
            rc_work: Rect::zero(),
            flags: 0,
        };
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return None;
        }
        if !is_bottom_bar(&bar, &info.rc_monitor) {
            return None;
        }
        if is_exclusive_fullscreen(&info.rc_monitor) {
            return None;
        }

        let tray = FindWindowExW(taskbar, 0, wide("TrayNotifyWnd").as_ptr(), std::ptr::null());
        let mut tray_left = bar.right;
        if tray != 0 {
            let mut tray_rect = Rect::zero();
            if GetWindowRect(tray, &mut tray_rect) != 0 {
                tray_left = tray_rect.left;
            }
        }

        let h = bar.bottom - bar.top;
        Some(Spot {
            tray_left,
            y: bar.top,
            h,
            dark_text: background_is_light(tray_left.saturating_sub(20), bar.top, h),
        })
    }
}

#[cfg(windows)]
fn is_bottom_bar(bar: &Rect, monitor: &Rect) -> bool {
    let w = bar.right - bar.left;
    let h = bar.bottom - bar.top;
    w > h.saturating_mul(2)
        && (24..=180).contains(&h)
        && bar.bottom >= monitor.bottom - 6
        && bar.top < monitor.bottom - 16
}

#[cfg(windows)]
fn is_exclusive_fullscreen(monitor: &Rect) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg == 0 || is_shell_window(fg) {
            return false;
        }
        if IsWindowVisible(fg) == 0 || IsIconic(fg) != 0 {
            return false;
        }
        let mut rc = Rect::zero();
        if GetWindowRect(fg, &mut rc) == 0 {
            return false;
        }
        let style = GetWindowLongPtrW(fg, GWL_STYLE) as u32;
        if style & WS_CAPTION != 0 {
            return false;
        }
        let mw = monitor.right - monitor.left;
        let mh = monitor.bottom - monitor.top;
        (rc.right - rc.left) >= mw - 4
            && (rc.bottom - rc.top) >= mh - 4
            && rc.left <= monitor.left + 4
            && rc.top <= monitor.top + 4
    }
}

#[cfg(windows)]
fn is_shell_window(hwnd: isize) -> bool {
    let class = class_name(hwnd);
    matches!(
        class.as_str(),
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "ForegroundStaging"
            | "Windows.UI.Core.CoreWindow"
            | "XamlExplorerHostIslandWindow"
            | "Windows.UI.Composition.DesktopWindowContentBridge"
            | "NotifyIconOverflowWindow"
            | "TrayNotifyWnd"
            | "DummyDWMListenerWindow"
    )
}

#[cfg(windows)]
fn class_name(hwnd: isize) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

#[cfg(windows)]
fn background_is_light(x: i32, y: i32, h: i32) -> bool {
    unsafe {
        let hdc = GetDC(0);
        if hdc == 0 {
            return false;
        }
        let sample_y = y + (h / 5).max(2);
        let mut lum = 0.0;
        let mut n = 0.0;
        for dx in [4, 14, 28] {
            let px = GetPixel(hdc, x + dx, sample_y);
            if px == 0xFFFF_FFFF {
                continue;
            }
            let r = (px & 0xFF) as f64;
            let g = ((px >> 8) & 0xFF) as f64;
            let b = ((px >> 16) & 0xFF) as f64;
            lum += 0.299 * r + 0.587 * g + 0.114 * b;
            n += 1.0;
        }
        ReleaseDC(0, hdc);
        n > 0.0 && lum / n > 170.0
    }
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(windows)]
impl Rect {
    fn zero() -> Self {
        Self {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        }
    }
}

#[cfg(windows)]
#[repr(C)]
struct MonitorInfo {
    cb_size: u32,
    rc_monitor: Rect,
    rc_work: Rect,
    flags: u32,
}

#[cfg(windows)]
const GWL_EXSTYLE: i32 = -20;
#[cfg(windows)]
const GWL_STYLE: i32 = -16;
#[cfg(windows)]
const GWLP_HWNDPARENT: i32 = -8;
#[cfg(windows)]
const WS_CAPTION: u32 = 0x00C0_0000;
#[cfg(windows)]
const WS_EX_LAYERED: isize = 0x0008_0000;
#[cfg(windows)]
const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
#[cfg(windows)]
const WS_EX_NOACTIVATE: isize = 0x0800_0000;
#[cfg(windows)]
const WS_EX_TRANSPARENT: isize = 0x0000_0020;
#[cfg(windows)]
const WS_EX_TOPMOST: isize = 0x0000_0008;
#[cfg(windows)]
const HWND_TOPMOST: isize = -1;
#[cfg(windows)]
const SWP_NOSIZE: u32 = 0x0001;
#[cfg(windows)]
const SWP_NOMOVE: u32 = 0x0002;
#[cfg(windows)]
const SWP_NOACTIVATE: u32 = 0x0010;
#[cfg(windows)]
const SWP_NOOWNERZORDER: u32 = 0x0200;
#[cfg(windows)]
const GWLP_WNDPROC: i32 = -4;
#[cfg(windows)]
const WM_WINDOWPOSCHANGING: u32 = 0x0046;
#[cfg(windows)]
const WM_SHOWWINDOW: u32 = 0x0018;
#[cfg(windows)]
const WM_SYSCOMMAND: u32 = 0x0112;
#[cfg(windows)]
const SC_MINIMIZE: usize = 0xF020;
#[cfg(windows)]
const GW_HWNDPREV: u32 = 3;
#[cfg(windows)]
const SWP_HIDEWINDOW: u32 = 0x0080;
#[cfg(windows)]
const SWP_SHOWWINDOW: u32 = 0x0040;
#[cfg(windows)]
const SW_SHOWNOACTIVATE: u32 = 8;
#[cfg(windows)]
const DWMWA_CLOAK: u32 = 13;
#[cfg(windows)]
const DWMWA_CLOAKED: u32 = 14;
#[cfg(windows)]
const MONITOR_DEFAULTTONEAREST: u32 = 2;

#[cfg(windows)]
#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[cfg(windows)]
#[repr(C)]
struct WindowPlacement {
    length: u32,
    flags: u32,
    show_cmd: u32,
    min_position: Point,
    max_position: Point,
    normal_position: Rect,
}

#[cfg(windows)]
#[repr(C)]
struct WindowPos {
    hwnd: isize,
    hwnd_insert_after: isize,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: u32,
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> isize;
    fn FindWindowExW(parent: isize, after: isize, class: *const u16, title: *const u16) -> isize;
    fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
    fn GetForegroundWindow() -> isize;
    fn GetClassNameW(hwnd: isize, class: *mut u16, max: i32) -> i32;
    fn IsWindowVisible(hwnd: isize) -> i32;
    fn IsIconic(hwnd: isize) -> i32;
    fn MonitorFromWindow(hwnd: isize, flags: u32) -> isize;
    fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
    fn GetDC(hwnd: isize) -> isize;
    fn ReleaseDC(hwnd: isize, hdc: isize) -> i32;
    fn GetPixel(hdc: isize, x: i32, y: i32) -> u32;
    fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
    fn SetWindowPos(
        hwnd: isize,
        insert_after: isize,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
    fn GetWindow(hwnd: isize, cmd: u32) -> isize;
    fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    fn GetWindowPlacement(hwnd: isize, place: *mut WindowPlacement) -> i32;
    fn SetWindowPlacement(hwnd: isize, place: *const WindowPlacement) -> i32;
    fn CallWindowProcW(prev: isize, hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
}

#[cfg(windows)]
#[link(name = "dwmapi")]
extern "system" {
    fn DwmGetWindowAttribute(hwnd: isize, attr: u32, value: *mut u32, size: u32) -> i32;
    fn DwmSetWindowAttribute(hwnd: isize, attr: u32, value: *const u32, size: u32) -> i32;
}
