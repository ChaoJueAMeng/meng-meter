use serde::Serialize;
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
            thread::sleep(Duration::from_millis(400));
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

    let Some(spot) = spot else {
        if state.shown {
            let _ = win.hide();
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
    let moved = state.x != x || state.y != spot.y || (state.h - logical_h).abs() > 0.5;
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
    raise_native(&win);

    state.tick = state.tick.wrapping_add(1);
    let theme_changed = state.theme.as_ref() != Some(&theme);
    if theme_changed || state.tick % 5 == 0 {
        let _ = win.emit("taskbar-theme", &theme);
        state.theme = Some(theme);
    }
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
            ex | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
        );
    }
}

#[cfg(not(windows))]
fn tune_native(_win: &WebviewWindow) {}

#[cfg(windows)]
fn raise_native(win: &WebviewWindow) {
    let Some(hwnd) = native_hwnd(win) else {
        return;
    };
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        let taskbar = FindWindowW(wide("Shell_TrayWnd").as_ptr(), std::ptr::null());
        if taskbar != 0 {
            SetWindowPos(
                taskbar,
                hwnd,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

#[cfg(not(windows))]
fn raise_native(_win: &WebviewWindow) {}

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
        if !is_bottom_bar(&bar, &info.rc_monitor) || foreground_is_fullscreen(taskbar, &info.rc_monitor)
        {
            return None;
        }

        let tray = FindWindowExW(taskbar, 0, wide("TrayNotifyWnd").as_ptr(), std::ptr::null());
        if tray == 0 {
            return None;
        }
        let mut tray_rect = Rect::zero();
        if GetWindowRect(tray, &mut tray_rect) == 0 {
            return None;
        }

        let h = bar.bottom - bar.top;
        Some(Spot {
            tray_left: tray_rect.left,
            y: bar.top,
            h,
            dark_text: background_is_light(tray_rect.left.saturating_sub(20), bar.top, h),
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
fn foreground_is_fullscreen(taskbar: isize, monitor: &Rect) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg == 0 || fg == taskbar {
            return false;
        }
        let mut rc = Rect::zero();
        if GetWindowRect(fg, &mut rc) == 0 {
            return false;
        }
        let mw = monitor.right - monitor.left;
        let mh = monitor.bottom - monitor.top;
        (rc.right - rc.left) >= mw - 8 && (rc.bottom - rc.top) >= mh - 8
    }
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
const WS_EX_LAYERED: isize = 0x0008_0000;
#[cfg(windows)]
const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
#[cfg(windows)]
const WS_EX_NOACTIVATE: isize = 0x0800_0000;
#[cfg(windows)]
const HWND_TOPMOST: isize = -1;
#[cfg(windows)]
const SWP_NOSIZE: u32 = 0x0001;
#[cfg(windows)]
const SWP_NOMOVE: u32 = 0x0002;
#[cfg(windows)]
const SWP_NOACTIVATE: u32 = 0x0010;
#[cfg(windows)]
const SWP_SHOWWINDOW: u32 = 0x0040;
#[cfg(windows)]
const MONITOR_DEFAULTTONEAREST: u32 = 2;

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> isize;
    fn FindWindowExW(parent: isize, after: isize, class: *const u16, title: *const u16) -> isize;
    fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
    fn GetForegroundWindow() -> isize;
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
}
