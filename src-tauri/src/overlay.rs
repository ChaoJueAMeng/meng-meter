use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::Duration;
use tauri::window::Color;
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, WebviewWindow};

use crate::settings::Settings;

const PAD: f64 = 10.0;
const BAR: f64 = 44.0;
const MIN_W: f64 = 88.0;
const TOOLBAR_MIN_W: f64 = 184.0;
const SIDE: f64 = 40.0;
const LOCK_W: f64 = 168.0;
const LOCK_H: f64 = 64.0;
const MIN_H: f64 = 52.0;
const MAX_W: f64 = 420.0;
const MAX_H: f64 = 220.0;

static LOCKED: AtomicBool = AtomicBool::new(true);
static DRAGGING: AtomicBool = AtomicBool::new(false);
static FILE: OnceLock<Mutex<OverlaysFile>> = OnceLock::new();

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OverlayGeom {
    x: i32,
    y: i32,
    w: f64,
    h: f64,
    color: String,
    opacity: f64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OverlaysFile {
    cpu: OverlayGeom,
    gpu: OverlayGeom,
    fps: OverlayGeom,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayView {
    pub kind: String,
    pub locked: bool,
    pub shown: bool,
    pub color: String,
    pub opacity: f64,
    pub w: f64,
    pub h: f64,
}

pub fn apply(app: &AppHandle, settings: &Settings) {
    LOCKED.store(settings.overlay_locked, Ordering::SeqCst);
    let file = file(app).clone();
    let present = settings.overlay_visible;
    place(
        app,
        "cpu",
        &file.cpu,
        settings.overlay_cpu,
        settings.overlay_locked,
        present,
    );
    place(
        app,
        "gpu",
        &file.gpu,
        settings.overlay_gpu,
        settings.overlay_locked,
        present,
    );
    place(
        app,
        "fps",
        &file.fps,
        settings.overlay_fps,
        settings.overlay_locked,
        present,
    );
    place_lock_button(app, settings.overlay_locked || !present);
}

pub fn view(app: &AppHandle, kind: &str, locked: bool, shown: bool) -> Option<OverlayView> {
    let geom = geom_of(app, kind)?;
    Some(OverlayView {
        kind: kind.to_string(),
        locked,
        shown,
        color: geom.color,
        opacity: geom.opacity,
        w: geom.w,
        h: geom.h,
    })
}

pub fn save_style(app: &AppHandle, kind: &str, color: &str, opacity: f64) -> Result<(), String> {
    if LOCKED.load(Ordering::SeqCst) {
        return Err("悬浮组件已锁定".into());
    }
    if window_label(kind).is_none() {
        return Err("未知悬浮组件".into());
    }
    if !valid_color(color) {
        return Err("颜色无效".into());
    }
    if !(0.25..=1.0).contains(&opacity) {
        return Err("透明度无效".into());
    }
    let color = color.to_ascii_lowercase();
    update(app, kind, |geom| {
        geom.color = color;
        geom.opacity = opacity;
    });
    Ok(())
}

pub fn begin_drag(app: AppHandle, kind: String) {
    if LOCKED.load(Ordering::SeqCst) || window_label(&kind).is_none() {
        return;
    }
    let Some(win) = window(&app, &kind) else {
        return;
    };
    let Ok(cursor0) = win.cursor_position() else {
        return;
    };
    let Ok(pos0) = win.outer_position() else {
        return;
    };
    let Some(geom) = geom_of(&app, &kind) else {
        return;
    };
    let content_w = geom.w;
    let content_h = geom.h;
    if DRAGGING.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(move || {
        let _guard = DragGuard;
        let c0x = cursor0.x as i32;
        let c0y = cursor0.y as i32;
        loop {
            thread::sleep(Duration::from_millis(8));
            if LOCKED.load(Ordering::SeqCst) || !crate::wininput::left_down() {
                break;
            }
            let app2 = app.clone();
            let kind2 = kind.clone();
            crate::on_main(&app, move || {
                let Some(win) = window(&app2, &kind2) else {
                    return;
                };
                let Ok(cursor) = win.cursor_position() else {
                    return;
                };
                let scale = win.scale_factor().unwrap_or(1.0).max(0.01);
                let pad = (PAD * scale).round() as i32;
                let inset = frame_left_px(content_w, scale);
                let box_w = (content_w * scale).round() as i32;
                let box_h = (content_h * scale).round() as i32;
                let x = pos0.x + cursor.x as i32 - c0x;
                let y = pos0.y + cursor.y as i32 - c0y;
                let (bx, by) = clamp_box_to_screen(&win, x + inset, y + pad, box_w, box_h, true);
                let _ = win.set_position(PhysicalPosition::new(bx - inset, by - pad));
            });
        }
        if !LOCKED.load(Ordering::SeqCst) {
            let app2 = app.clone();
            let kind2 = kind.clone();
            crate::on_main(&app, move || commit_origin(&app2, &kind2));
        }
    });
}

pub fn begin_resize(app: AppHandle, kind: String, corner: String) {
    if LOCKED.load(Ordering::SeqCst) || window_label(&kind).is_none() {
        return;
    }
    if !matches!(
        corner.as_str(),
        "nw" | "ne" | "sw" | "se" | "n" | "s" | "e" | "w"
    ) {
        return;
    }
    let Some(win) = window(&app, &kind) else {
        return;
    };
    let Ok(cursor0) = win.cursor_position() else {
        return;
    };
    let Ok(pos0) = win.outer_position() else {
        return;
    };
    let Some(geom) = geom_of(&app, &kind) else {
        return;
    };
    let scale0 = win.scale_factor().unwrap_or(1.0).max(0.01);
    let pad0 = (PAD * scale0).round() as i32;
    let fixed_left = pos0.x + frame_left_px(geom.w, scale0);
    let fixed_top = pos0.y + pad0;
    let fixed_right = fixed_left + (geom.w * scale0).round() as i32;
    let fixed_bottom = fixed_top + (geom.h * scale0).round() as i32;
    if DRAGGING.swap(true, Ordering::SeqCst) {
        return;
    }
    let latest = Arc::new(Mutex::new((geom.w, geom.h, fixed_left, fixed_top)));
    thread::spawn(move || {
        let _guard = DragGuard;
        let c0x = cursor0.x as i32;
        let c0y = cursor0.y as i32;
        loop {
            thread::sleep(Duration::from_millis(8));
            if LOCKED.load(Ordering::SeqCst) || !crate::wininput::left_down() {
                break;
            }
            let app2 = app.clone();
            let kind2 = kind.clone();
            let corner2 = corner.clone();
            let latest2 = latest.clone();
            crate::on_main(&app, move || {
                let Some(win) = window(&app2, &kind2) else {
                    return;
                };
                let Ok(cursor) = win.cursor_position() else {
                    return;
                };
                let scale = win.scale_factor().unwrap_or(scale0).max(0.01);
                let dx = cursor.x as i32 - c0x;
                let dy = cursor.y as i32 - c0y;
                let (left, top, right, bottom) = match corner2.as_str() {
                    "se" => (fixed_left, fixed_top, fixed_right + dx, fixed_bottom + dy),
                    "nw" => (fixed_left + dx, fixed_top + dy, fixed_right, fixed_bottom),
                    "ne" => (fixed_left, fixed_top + dy, fixed_right + dx, fixed_bottom),
                    "sw" => (fixed_left + dx, fixed_top, fixed_right, fixed_bottom + dy),
                    "n" => (fixed_left, fixed_top + dy, fixed_right, fixed_bottom),
                    "s" => (fixed_left, fixed_top, fixed_right, fixed_bottom + dy),
                    "e" => (fixed_left, fixed_top, fixed_right + dx, fixed_bottom),
                    "w" => (fixed_left + dx, fixed_top, fixed_right, fixed_bottom),
                    _ => return,
                };
                let (sx, sy, sw, sh) =
                    screen_rect(&win, (left + right) / 2, (top + bottom) / 2, true);
                let (left, top, right, bottom) =
                    clamp_resize_edges(corner2.as_str(), left, top, right, bottom, sx, sy, sw, sh);
                let w = ((right - left) as f64 / scale).round().clamp(MIN_W, MAX_W);
                let h = ((bottom - top) as f64 / scale).round().clamp(MIN_H, MAX_H);
                let wp = (w * scale).round() as i32;
                let hp = (h * scale).round() as i32;
                let (cx, cy) = match corner2.as_str() {
                    "nw" => (fixed_right - wp, fixed_bottom - hp),
                    "ne" => (fixed_left, fixed_bottom - hp),
                    "sw" => (fixed_right - wp, fixed_top),
                    "n" => (fixed_left, fixed_bottom - hp),
                    "w" => (fixed_right - wp, fixed_top),
                    _ => (fixed_left, fixed_top),
                };
                if let Ok(mut slot) = latest2.lock() {
                    *slot = (w, h, cx, cy);
                }
                let pad = (PAD * scale).round() as i32;
                let (ow, oh) = outer_size(w, h, false);
                sync_box(&win, w, h);
                let _ = win.set_position(PhysicalPosition::new(
                    cx - frame_left_px(w, scale),
                    cy - pad,
                ));
                let _ = win.set_size(LogicalSize::new(ow, oh));
            });
        }
        if !LOCKED.load(Ordering::SeqCst) {
            let app2 = app.clone();
            let kind2 = kind.clone();
            let latest2 = latest.clone();
            crate::on_main(&app, move || {
                let Ok(slot) = latest2.lock() else {
                    return;
                };
                let (w, h, x, y) = *slot;
                drop(slot);
                commit_box(&app2, &kind2, x, y, w, h);
            });
        }
    });
}

struct DragGuard;

impl Drop for DragGuard {
    fn drop(&mut self) {
        DRAGGING.store(false, Ordering::SeqCst);
    }
}

fn on_screen(saved: bool, locked: bool, present: bool) -> bool {
    present && (saved || !locked)
}

fn place(
    app: &AppHandle,
    kind: &str,
    geom: &OverlayGeom,
    visible: bool,
    locked: bool,
    present: bool,
) {
    let Some(win) = window(app, kind) else {
        return;
    };
    let scale = win.scale_factor().unwrap_or(1.0).max(0.01);
    let box_w = (geom.w * scale).round() as i32;
    let box_h = (geom.h * scale).round() as i32;
    let (bx, by) = clamp_box_to_screen(&win, geom.x, geom.y, box_w, box_h, false);
    let (x, y) = outer_origin(bx, by, geom.w, scale, locked);
    let (w, h) = outer_size(geom.w, geom.h, locked);
    sync_page(&win, kind, locked, visible, geom);
    let _ = win.set_ignore_cursor_events(locked || !present);
    let _ = win.set_size(LogicalSize::new(w, h));
    let _ = win.set_position(PhysicalPosition::new(x, y));
    if on_screen(visible, locked, present) {
        let _ = win.set_background_color(Some(Color(0, 0, 0, 0)));
        let _ = win.show();
        let _ = win.set_always_on_top(true);
    } else {
        let _ = win.hide();
    }
}

fn sync_page(win: &WebviewWindow, kind: &str, locked: bool, shown: bool, geom: &OverlayGeom) {
    let mode = if locked { "on" } else { "off" };
    let shown_mode = if shown { "on" } else { "off" };
    let color = if valid_color(&geom.color) {
        geom.color.as_str()
    } else {
        "#ffffff"
    };
    let _ = win.eval(format!(
        "document.documentElement.dataset.kind='{kind}';document.documentElement.dataset.locked='{mode}';document.documentElement.dataset.shown='{shown_mode}';document.documentElement.style.setProperty('--ink','{color}');document.documentElement.style.setProperty('--fade','{opacity}');document.documentElement.style.setProperty('--box-w','{w}px');document.documentElement.style.setProperty('--box-h','{h}px');",
        opacity = geom.opacity,
        w = geom.w,
        h = geom.h
    ));
}

fn place_lock_button(app: &AppHandle, locked: bool) {
    let Some(win) = app.get_webview_window("layout-lock") else {
        return;
    };
    if locked {
        let _ = win.hide();
        return;
    }
    let monitor = app.primary_monitor().ok().flatten();
    let scale = monitor
        .as_ref()
        .map(|item| item.scale_factor())
        .unwrap_or(1.0)
        .max(1.0);
    let area = monitor.as_ref().map(|item| item.work_area());
    let (ax, ay, aw) = match area {
        Some(area) => (area.position.x, area.position.y, area.size.width as i32),
        None => (0, 0, 1920),
    };
    let width = (LOCK_W * scale).round() as i32;
    let margin = (12.0 * scale).round() as i32;
    let x = ax + (aw - width) / 2;
    let y = ay + margin;
    let _ = win.set_size(LogicalSize::new(LOCK_W, LOCK_H));
    let _ = win.set_position(PhysicalPosition::new(x, y));
    let _ = win.set_ignore_cursor_events(false);
    let _ = win.set_background_color(Some(Color(0, 0, 0, 0)));
    let _ = win.show();
    let _ = win.set_always_on_top(true);
}

fn sync_box(win: &WebviewWindow, w: f64, h: f64) {
    let _ = win.eval(format!(
        "document.documentElement.style.setProperty('--box-w','{w}px');document.documentElement.style.setProperty('--box-h','{h}px');"
    ));
}

fn commit_origin(app: &AppHandle, kind: &str) {
    if LOCKED.load(Ordering::SeqCst) {
        return;
    }
    let Some(win) = window(app, kind) else {
        return;
    };
    let Ok(pos) = win.outer_position() else {
        return;
    };
    let Some(current) = geom_of(app, kind) else {
        return;
    };
    let scale = win.scale_factor().unwrap_or(1.0).max(0.01);
    let pad = (PAD * scale).round() as i32;
    let inset = frame_left_px(current.w, scale);
    let box_w = (current.w * scale).round() as i32;
    let box_h = (current.h * scale).round() as i32;
    let (x, y) = clamp_box_to_screen(&win, pos.x + inset, pos.y + pad, box_w, box_h, false);
    update(app, kind, |geom| {
        geom.x = x;
        geom.y = y;
    });
}

fn commit_box(app: &AppHandle, kind: &str, mut x: i32, mut y: i32, w: f64, h: f64) {
    if LOCKED.load(Ordering::SeqCst) {
        return;
    }
    if let Some(win) = window(app, kind) {
        let scale = win.scale_factor().unwrap_or(1.0).max(0.01);
        let box_w = (w * scale).round() as i32;
        let box_h = (h * scale).round() as i32;
        (x, y) = clamp_box_to_screen(&win, x, y, box_w, box_h, false);
    }
    update(app, kind, |geom| {
        geom.x = x;
        geom.y = y;
        geom.w = w;
        geom.h = h;
    });
    let Some(geom) = geom_of(app, kind) else {
        return;
    };
    let Some(win) = window(app, kind) else {
        return;
    };
    let scale = win.scale_factor().unwrap_or(1.0).max(0.01);
    let (ox, oy) = outer_origin(geom.x, geom.y, geom.w, scale, false);
    let (ow, oh) = outer_size(geom.w, geom.h, false);
    sync_box(&win, geom.w, geom.h);
    let _ = win.set_position(PhysicalPosition::new(ox, oy));
    let _ = win.set_size(LogicalSize::new(ow, oh));
}

fn outer_width(content_w: f64) -> f64 {
    (content_w + SIDE * 2.0).max(TOOLBAR_MIN_W)
}

fn frame_left(content_w: f64) -> f64 {
    (outer_width(content_w) - content_w) / 2.0
}

fn frame_left_px(content_w: f64, scale: f64) -> i32 {
    (frame_left(content_w) * scale).round() as i32
}

fn outer_origin(x: i32, y: i32, content_w: f64, scale: f64, locked: bool) -> (i32, i32) {
    if locked {
        (x, y)
    } else {
        let pad = (PAD * scale).round() as i32;
        (x - frame_left_px(content_w, scale), y - pad)
    }
}

fn outer_size(w: f64, h: f64, locked: bool) -> (f64, f64) {
    if locked {
        (w, h)
    } else {
        (outer_width(w), h + PAD + BAR)
    }
}

fn clamp_box_to_screen(
    win: &WebviewWindow,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    follow_cursor: bool,
) -> (i32, i32) {
    let (sx, sy, sw, sh) = screen_rect(win, x + w / 2, y + h / 2, follow_cursor);
    clamp_origin(x, y, w, h, sx, sy, sw, sh)
}

fn screen_rect(win: &WebviewWindow, x: i32, y: i32, follow_cursor: bool) -> (i32, i32, i32, i32) {
    let cursor_mon = if follow_cursor {
        win.cursor_position()
            .ok()
            .and_then(|cursor| win.monitor_from_point(cursor.x, cursor.y).ok().flatten())
    } else {
        None
    };
    let monitor = cursor_mon
        .or_else(|| win.monitor_from_point(x as f64, y as f64).ok().flatten())
        .or_else(|| win.current_monitor().ok().flatten())
        .or_else(|| win.primary_monitor().ok().flatten());
    match monitor {
        Some(monitor) => {
            let pos = monitor.position();
            let size = monitor.size();
            (pos.x, pos.y, size.width as i32, size.height as i32)
        }
        None => (0, 0, 1920, 1080),
    }
}

fn clamp_origin(x: i32, y: i32, w: i32, h: i32, sx: i32, sy: i32, sw: i32, sh: i32) -> (i32, i32) {
    let max_x = (sx + sw - w).max(sx);
    let max_y = (sy + sh - h).max(sy);
    (x.clamp(sx, max_x), y.clamp(sy, max_y))
}

fn clamp_resize_edges(
    corner: &str,
    mut left: i32,
    mut top: i32,
    mut right: i32,
    mut bottom: i32,
    sx: i32,
    sy: i32,
    sw: i32,
    sh: i32,
) -> (i32, i32, i32, i32) {
    let sr = sx + sw;
    let sb = sy + sh;
    match corner {
        "e" | "se" | "ne" => {
            right = right.clamp(left, sr.max(left));
        }
        "w" | "sw" | "nw" => {
            left = left.clamp(sx.min(right), right);
        }
        _ => {}
    }
    match corner {
        "s" | "se" | "sw" => {
            bottom = bottom.clamp(top, sb.max(top));
        }
        "n" | "ne" | "nw" => {
            top = top.clamp(sy.min(bottom), bottom);
        }
        _ => {}
    }
    (left, top, right, bottom)
}

fn window<'a>(app: &'a AppHandle, kind: &str) -> Option<WebviewWindow> {
    app.get_webview_window(window_label(kind)?)
}

fn window_label(kind: &str) -> Option<&'static str> {
    match kind {
        "cpu" => Some("overlay-cpu"),
        "gpu" => Some("overlay-gpu"),
        "fps" => Some("overlay-fps"),
        _ => None,
    }
}

fn geom_of(app: &AppHandle, kind: &str) -> Option<OverlayGeom> {
    let file = file(app);
    match kind {
        "cpu" => Some(file.cpu.clone()),
        "gpu" => Some(file.gpu.clone()),
        "fps" => Some(file.fps.clone()),
        _ => None,
    }
}

fn update(app: &AppHandle, kind: &str, f: impl FnOnce(&mut OverlayGeom)) {
    let mut file = file_mut(app);
    let geom = match kind {
        "gpu" => &mut file.gpu,
        "fps" => &mut file.fps,
        _ => &mut file.cpu,
    };
    f(geom);
    sanitize_geom(geom);
    let snapshot = file.clone();
    drop(file);
    write(app, &snapshot);
}

fn file(app: &AppHandle) -> MutexGuard<'static, OverlaysFile> {
    file_mut(app)
}

fn file_mut(app: &AppHandle) -> MutexGuard<'static, OverlaysFile> {
    FILE.get_or_init(|| Mutex::new(load(app)))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

fn load(app: &AppHandle) -> OverlaysFile {
    if let Some(path) = store_path(app) {
        if let Ok(bytes) = fs::read(&path) {
            if let Ok(mut file) = serde_json::from_slice::<OverlaysFile>(&bytes) {
                sanitize(&mut file);
                return file;
            }
        }
    }
    let file = fallback(app);
    write(app, &file);
    file
}

fn write(app: &AppHandle, file: &OverlaysFile) {
    let Some(path) = store_path(app) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(file) {
        let _ = fs::write(path, bytes);
    }
}

fn store_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    Some(dir.join("overlays.json"))
}

fn fallback(app: &AppHandle) -> OverlaysFile {
    let monitor = app.primary_monitor().ok().flatten();
    let scale = monitor
        .as_ref()
        .map(|monitor| monitor.scale_factor())
        .unwrap_or(1.0)
        .max(1.0);
    let area = monitor.as_ref().map(|monitor| monitor.work_area());
    let (ax, ay, aw) = match area {
        Some(area) => (area.position.x, area.position.y, area.size.width as i32),
        None => (0, 0, 1920),
    };
    let w = 128.0;
    let h = 72.0;
    let pw = (w * scale).round() as i32;
    let ph = (h * scale).round() as i32;
    let margin = (24.0 * scale).round() as i32;
    let gap = (16.0 * scale).round() as i32;
    let x = ax + aw - pw - margin;
    let y = ay + margin;
    let geom = |row: i32| OverlayGeom {
        x,
        y: y + row * (ph + gap),
        w,
        h,
        color: "#ffffff".to_string(),
        opacity: 0.92,
    };
    OverlaysFile {
        cpu: geom(0),
        gpu: geom(1),
        fps: geom(2),
    }
}

fn sanitize(file: &mut OverlaysFile) {
    sanitize_geom(&mut file.cpu);
    sanitize_geom(&mut file.gpu);
    sanitize_geom(&mut file.fps);
}

fn sanitize_geom(geom: &mut OverlayGeom) {
    if !geom.w.is_finite() {
        geom.w = 128.0;
    }
    if !geom.h.is_finite() {
        geom.h = 72.0;
    }
    geom.w = geom.w.clamp(MIN_W, MAX_W);
    geom.h = geom.h.clamp(MIN_H, MAX_H);
    if !geom.opacity.is_finite() {
        geom.opacity = 0.92;
    }
    geom.opacity = geom.opacity.clamp(0.25, 1.0);
    if !valid_color(&geom.color) {
        geom.color = "#ffffff".to_string();
    } else {
        geom.color = geom.color.to_ascii_lowercase();
    }
}

fn valid_color(color: &str) -> bool {
    let bytes = color.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_origin, clamp_resize_edges, frame_left, on_screen, outer_size, BAR, MIN_W, PAD,
        TOOLBAR_MIN_W,
    };

    #[test]
    fn master_hide_keeps_saved_components_off_screen() {
        assert!(!on_screen(true, true, false));
        assert!(!on_screen(true, false, false));
        assert!(!on_screen(false, false, false));
        assert!(on_screen(true, true, true));
        assert!(on_screen(false, false, true));
        assert!(!on_screen(false, true, true));
    }

    #[test]
    fn narrow_frame_stays_narrow() {
        assert_eq!(MIN_W, 88.0);
        let (locked_w, locked_h) = outer_size(88.0, 52.0, true);
        assert_eq!((locked_w, locked_h), (88.0, 52.0));
        let (w, h) = outer_size(88.0, 52.0, false);
        assert_eq!(w, TOOLBAR_MIN_W);
        assert_eq!(h, 52.0 + PAD + BAR);
        let (wide, _) = outer_size(300.0, 72.0, false);
        assert_eq!(wide, 300.0 + super::SIDE * 2.0);
        assert_eq!(frame_left(88.0), (TOOLBAR_MIN_W - 88.0) / 2.0);
        assert_eq!(frame_left(300.0), super::SIDE);
    }

    #[test]
    fn drag_keeps_box_inside_screen() {
        let screen = (0, 0, 1920, 1080);
        assert_eq!(
            clamp_origin(100, 80, 128, 72, screen.0, screen.1, screen.2, screen.3),
            (100, 80)
        );
        assert_eq!(
            clamp_origin(-40, -20, 128, 72, screen.0, screen.1, screen.2, screen.3),
            (0, 0)
        );
        assert_eq!(
            clamp_origin(1900, 1070, 128, 72, screen.0, screen.1, screen.2, screen.3),
            (1792, 1008)
        );
        assert_eq!(
            clamp_origin(-200, 40, 128, 72, -1920, 0, 1920, 1080),
            (-200, 40)
        );
        assert_eq!(
            clamp_origin(-2000, 40, 128, 72, -1920, 0, 1920, 1080),
            (-1920, 40)
        );
    }

    #[test]
    fn resize_keeps_moving_edges_inside_screen() {
        let (left, top, right, bottom) =
            clamp_resize_edges("se", 100, 80, 2200, 1400, 0, 0, 1920, 1080);
        assert_eq!((left, top, right, bottom), (100, 80, 1920, 1080));
        let (left, top, right, bottom) =
            clamp_resize_edges("nw", -80, -40, 200, 160, 0, 0, 1920, 1080);
        assert_eq!((left, top, right, bottom), (0, 0, 200, 160));
        let (left, top, right, bottom) =
            clamp_resize_edges("e", 100, 80, 3000, 160, 0, 0, 1920, 1080);
        assert_eq!((left, top, right, bottom), (100, 80, 1920, 160));
    }
}
