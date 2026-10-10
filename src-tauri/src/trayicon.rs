use tauri::image::Image;

pub const ID: &str = "meter";
pub const SAMPLES: usize = 28;

const SIZE: u32 = 64;

#[derive(Clone, Copy)]
struct PlotPoint {
    x: f32,
    y: f32,
    percent: f32,
}

pub fn color_for(percent: f64) -> (u8, u8, u8) {
    if percent < 50.0 {
        (34, 197, 94)
    } else if percent < 80.0 {
        (245, 158, 11)
    } else {
        (239, 68, 68)
    }
}

pub fn render(history: &[f64]) -> Image<'static> {
    let size = SIZE as usize;
    let mut rgba = vec![0u8; size * size * 4];

    let s = SIZE as f32;
    let frame_min = 1.5;
    let frame_max = s - 1.5;
    let radius = 7.0;
    let plot_l = 8.0;
    let plot_r = s - 8.0;
    let plot_t = 8.5;
    let plot_b = s - 8.5;
    let points = plot_points(history, plot_l, plot_r, plot_t, plot_b);

    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let frame = cover(-rbox(fx, fy, frame_min, frame_min, frame_max, frame_max, radius));
            if frame <= 0.0 {
                continue;
            }

            blend(&mut rgba, x, y, (12, 13, 16), frame);

            let inner = cover(-rbox(
                fx,
                fy,
                plot_l - 1.2,
                plot_t - 1.2,
                plot_r + 1.2,
                plot_b + 1.2,
                3.2,
            ));
            if inner > 0.0 {
                blend(&mut rgba, x, y, (8, 9, 12), inner * frame);
            }

            let in_plot = fx >= plot_l && fx <= plot_r && fy >= plot_t && fy <= plot_b + 0.5;
            if in_plot {
                if let Some((ly, percent)) = sample_at(&points, fx) {
                    let cover_px = (fy - (ly - 0.9) + 0.5).clamp(0.0, 1.0);
                    if cover_px > 0.0 {
                        blend(&mut rgba, x, y, color_for(percent as f64), cover_px);
                    }
                }
            }

            let sdf = rbox(fx, fy, frame_min, frame_min, frame_max, frame_max, radius);
            let edge = (0.9 - sdf.abs()).clamp(0.0, 1.0);
            if edge > 0.0 {
                blend(&mut rgba, x, y, (46, 48, 56), edge * 0.95);
            }
        }
    }

    Image::new_owned(rgba, SIZE, SIZE)
}

fn plot_points(history: &[f64], left: f32, right: f32, top: f32, bottom: f32) -> Vec<PlotPoint> {
    let n = history.len().min(SAMPLES);
    if n == 0 {
        return Vec::new();
    }
    let start = SAMPLES - n;
    let src = &history[history.len() - n..];
    let mut points: Vec<PlotPoint> = src
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let percent = (*v as f32).clamp(0.0, 100.0);
            let slot = start + i;
            PlotPoint {
                x: left + (right - left) * slot as f32 / (SAMPLES - 1) as f32,
                y: bottom - (bottom - top) * (percent / 100.0),
                percent,
            }
        })
        .collect();
    if points.len() == 1 {
        let slot_w = (right - left) / (SAMPLES - 1) as f32;
        let p = points[0];
        points.insert(
            0,
            PlotPoint {
                x: (p.x - slot_w).max(left),
                y: p.y,
                percent: p.percent,
            },
        );
    }
    points
}

fn sample_at(points: &[PlotPoint], x: f32) -> Option<(f32, f32)> {
    for pair in points.windows(2) {
        let a = &pair[0];
        let b = &pair[1];
        if x + 0.001 < a.x || x - 0.001 > b.x {
            continue;
        }
        let span = b.x - a.x;
        let t = if span < 0.001 {
            0.0
        } else {
            ((x - a.x) / span).clamp(0.0, 1.0)
        };
        let y = a.y + (b.y - a.y) * t;
        let percent = a.percent + (b.percent - a.percent) * t;
        return Some((y, percent));
    }
    None
}

fn rbox(px: f32, py: f32, minx: f32, miny: f32, maxx: f32, maxy: f32, r: f32) -> f32 {
    let cx = (minx + maxx) * 0.5;
    let cy = (miny + maxy) * 0.5;
    let hw = (maxx - minx) * 0.5;
    let hh = (maxy - miny) * 0.5;
    let dx = (px - cx).abs() - (hw - r);
    let dy = (py - cy).abs() - (hh - r);
    dx.max(0.0).hypot(dy.max(0.0)) + dx.min(0.0).max(dy.min(0.0)) - r
}

fn cover(signed: f32) -> f32 {
    (0.55 + signed).clamp(0.0, 1.0)
}

fn blend(buf: &mut [u8], x: usize, y: usize, rgb: (u8, u8, u8), alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let i = (y * SIZE as usize + x) * 4;
    if i + 3 >= buf.len() {
        return;
    }
    let da = buf[i + 3] as f32 / 255.0;
    let out_a = a + da * (1.0 - a);
    if out_a <= 0.0 {
        return;
    }
    let over = |s: u8, d: u8| ((s as f32 * a + d as f32 * da * (1.0 - a)) / out_a).round() as u8;
    buf[i] = over(rgb.0, buf[i]);
    buf[i + 1] = over(rgb.1, buf[i + 1]);
    buf[i + 2] = over(rgb.2, buf[i + 2]);
    buf[i + 3] = (out_a * 255.0).round() as u8;
}
