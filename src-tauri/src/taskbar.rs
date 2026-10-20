use tauri::AppHandle;

pub fn set_sources(cpu: bool, gpu: bool) {
    #[cfg(windows)]
    platform::set_sources(cpu, gpu);
    #[cfg(not(windows))]
    {
        let _ = (cpu, gpu);
    }
}

pub fn spawn(_app: AppHandle) {
    #[cfg(windows)]
    platform::spawn();
}

#[cfg(windows)]
mod platform {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;
    use std::thread;
    use std::time::{Duration, Instant};

    static CPU_ON: AtomicBool = AtomicBool::new(false);
    static GPU_ON: AtomicBool = AtomicBool::new(false);
    static ALLOW_HIDE: AtomicBool = AtomicBool::new(false);

    const CLASS_NAME: &str = "MengMeterTaskbar";
    const HOT_CELSIUS: i64 = 95;
    const COLOR_DARK: u32 = 0x0033_3333;
    const COLOR_LIGHT: u32 = 0x00FF_FFFF;
    const COLOR_HOT: u32 = 0x0033_33FF;
    const DT_CENTER: u32 = 0x0001;
    const DT_VCENTER: u32 = 0x0004;
    const DT_SINGLELINE: u32 = 0x0020;
    const DT_NOPREFIX: u32 = 0x0800;
    const DT_NOCLIP: u32 = 0x0100;
    const DT_CALCRECT: u32 = 0x0400;
    const DT_DRAW: u32 = DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_NOCLIP;
    const DT_MEASURE: u32 = DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX;
    const DTT_TEXTCOLOR: u32 = 0x0001;
    const DTT_COMPOSITED: u32 = 0x2000;
    const FW_NORMAL: i32 = 400;
    const FW_SEMIBOLD: i32 = 600;
    const GB2312_CHARSET: u32 = 134;
    const DEFAULT_CHARSET: u32 = 1;
    const ANTIALIASED_QUALITY: u32 = 4;
    const SYSTEM_FONT: i32 = 13;
    const TRANSPARENT: i32 = 1;
    const LOGPIXELSY: i32 = 90;
    const ULW_ALPHA: u32 = 2;
    const AC_SRC_OVER: u8 = 0;
    const AC_SRC_ALPHA: u8 = 1;
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
    const WS_CLIPCHILDREN: u32 = 0x0200_0000;
    const WS_EX_LAYERED: u32 = 0x0008_0000;
    const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
    const WS_EX_TOPMOST: u32 = 0x0000_0008;
    const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
    const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
    const GWL_STYLE: i32 = -16;
    const GWLP_HWNDPARENT: i32 = -8;
    const WS_CAPTION: u32 = 0x00C0_0000;
    const HWND_TOPMOST: isize = -1;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_SHOWWINDOW: u32 = 0x0040;
    const SWP_HIDEWINDOW: u32 = 0x0080;
    const SWP_NOOWNERZORDER: u32 = 0x0200;
    const SWP_NOZORDER: u32 = 0x0004;
    const GW_HWNDPREV: u32 = 3;
    const SW_HIDE: i32 = 0;
    const SW_SHOWNOACTIVATE: i32 = 8;
    const WM_WINDOWPOSCHANGING: u32 = 0x0046;
    const WM_SHOWWINDOW: u32 = 0x0018;
    const WM_SYSCOMMAND: u32 = 0x0112;
    const SC_MINIMIZE: usize = 0xF020;
    const DWMWA_CLOAK: u32 = 13;
    const DWMWA_CLOAKED: u32 = 14;
    const MONITOR_DEFAULTTONEAREST: u32 = 2;
    const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
    const KEY_QUERY_VALUE: u32 = 0x0001;
    const REG_DWORD: u32 = 4;
    const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;

    pub fn set_sources(cpu: bool, gpu: bool) {
        CPU_ON.store(cpu, Ordering::SeqCst);
        GPU_ON.store(gpu, Ordering::SeqCst);
    }

    pub fn spawn() {
        thread::Builder::new()
            .name("meng-taskbar".into())
            .spawn(run)
            .ok();
    }

    fn run() {
        let mut live = Live::default();
        loop {
            drain_messages();
            if CPU_ON.load(Ordering::SeqCst) || GPU_ON.load(Ordering::SeqCst) {
                live.tick();
            } else {
                live.destroy();
            }
            thread::sleep(Duration::from_millis(120));
        }
    }

    #[derive(Default)]
    struct Live {
        hwnd: isize,
        shown: bool,
        frame: Option<Frame>,
        measured: Option<Measured>,
        probe: Option<crate::cputemp::Probe>,
        celsius: Option<f64>,
        sampled: Option<Instant>,
    }

    #[derive(Clone, PartialEq, Eq)]
    struct Col {
        text: String,
        label: String,
        color: u32,
    }

    #[derive(Clone, PartialEq, Eq)]
    struct Frame {
        cols: Vec<Col>,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        dpi: i32,
    }

    struct Measured {
        cols: Vec<(String, String)>,
        dpi: i32,
        tray_h: i32,
        layout: Layout,
    }

    #[derive(Clone)]
    struct Layout {
        widths: Vec<i32>,
        col_gap: i32,
        w: i32,
        temp_px: i32,
        label_px: i32,
        temp_h: i32,
        label_h: i32,
        gap_y: i32,
    }

    impl Live {
        fn tick(&mut self) {
            self.forget_dead_window();
            let Some(spot) = measure() else {
                self.hide();
                return;
            };
            if CPU_ON.load(Ordering::SeqCst) {
                self.sample_if_due();
            }
            let light = system_uses_light_theme().unwrap_or_else(|| {
                background_is_light(spot.tray_left.saturating_add(8), spot.y, spot.h)
            });
            let mut cols = Vec::new();
            if GPU_ON.load(Ordering::SeqCst) {
                let shown = rounded_celsius(crate::gpu::selected_celsius());
                cols.push(Col {
                    text: temp_text(shown),
                    label: "GPU温度".to_string(),
                    color: text_color(shown, light),
                });
            }
            if CPU_ON.load(Ordering::SeqCst) {
                let shown = rounded_celsius(self.celsius);
                cols.push(Col {
                    text: temp_text(shown),
                    label: "CPU温度".to_string(),
                    color: text_color(shown, light),
                });
            }
            if cols.is_empty() {
                self.hide();
                return;
            }
            let layout = self.layout_for(&cols, spot.dpi, spot.h);
            let frame = Frame {
                cols,
                x: spot
                    .tray_left
                    .saturating_sub(layout.w)
                    .saturating_sub(mm_to_px(spot.dpi, 1)),
                y: spot.y,
                w: layout.w,
                h: spot.h,
                dpi: spot.dpi,
            };
            if self.hwnd == 0 {
                self.hwnd = create_window(frame.x, frame.y, frame.w, frame.h);
                if self.hwnd == 0 {
                    return;
                }
            }
            if self
                .frame
                .as_ref()
                .is_some_and(|prev| same_pixels(prev, &frame))
            {
                if self
                    .frame
                    .as_ref()
                    .is_some_and(|prev| prev.x != frame.x || prev.y != frame.y)
                {
                    move_window(self.hwnd, frame.x, frame.y);
                }
                self.frame = Some(frame);
                self.reveal();
                return;
            }
            if paint(self.hwnd, &frame, &layout) {
                self.frame = Some(frame);
                self.reveal();
            }
        }

        fn forget_dead_window(&mut self) {
            if self.hwnd != 0 && unsafe { IsWindow(self.hwnd) } == 0 {
                self.hwnd = 0;
                self.shown = false;
                self.frame = None;
            }
        }

        fn sample_if_due(&mut self) {
            let due = self
                .sampled
                .map(|at| at.elapsed() >= Duration::from_secs(1))
                .unwrap_or(true);
            if !due {
                return;
            }
            if self.probe.is_none() {
                self.probe = crate::cputemp::Probe::open();
            }
            self.celsius = self.probe.as_ref().and_then(|probe| probe.celsius());
            self.sampled = Some(Instant::now());
        }

        fn layout_for(&mut self, cols: &[Col], dpi: i32, tray_h: i32) -> Layout {
            let key: Vec<(String, String)> = cols
                .iter()
                .map(|col| (col.text.clone(), col.label.clone()))
                .collect();
            if let Some(prev) = &self.measured {
                if prev.cols == key && prev.dpi == dpi && prev.tray_h == tray_h {
                    return prev.layout.clone();
                }
            }
            let layout = layout_columns(cols, dpi, tray_h);
            self.measured = Some(Measured {
                cols: key,
                dpi,
                tray_h,
                layout: layout.clone(),
            });
            layout
        }

        fn hide(&mut self) {
            if self.hwnd == 0 || !self.shown {
                return;
            }
            ALLOW_HIDE.store(true, Ordering::SeqCst);
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            ALLOW_HIDE.store(false, Ordering::SeqCst);
            self.shown = false;
        }

        fn reveal(&mut self) {
            if self.hwnd == 0 {
                return;
            }
            if !self.shown {
                unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
                self.shown = true;
            }
            keep_visible(self.hwnd);
        }

        fn destroy(&mut self) {
            if self.hwnd != 0 {
                unsafe {
                    if IsWindow(self.hwnd) != 0 {
                        ALLOW_HIDE.store(true, Ordering::SeqCst);
                        DestroyWindow(self.hwnd);
                        ALLOW_HIDE.store(false, Ordering::SeqCst);
                    }
                }
                self.hwnd = 0;
            }
            self.shown = false;
            self.frame = None;
        }
    }

    fn same_pixels(prev: &Frame, next: &Frame) -> bool {
        prev.cols == next.cols && prev.w == next.w && prev.h == next.h && prev.dpi == next.dpi
    }

    fn rounded_celsius(celsius: Option<f64>) -> Option<i64> {
        let value = celsius.filter(|value| value.is_finite())?;
        Some(value.round() as i64)
    }

    fn temp_text(shown: Option<i64>) -> String {
        match shown {
            Some(value) => format!("{value}℃"),
            None => "--°C".to_string(),
        }
    }

    fn text_color(shown: Option<i64>, light: bool) -> u32 {
        if shown.is_some_and(|value| value >= HOT_CELSIUS) {
            COLOR_HOT
        } else if light {
            COLOR_DARK
        } else {
            COLOR_LIGHT
        }
    }

    fn move_window(hwnd: isize, x: i32, y: i32) {
        unsafe {
            let flags = SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
            let insert = if menu_is_open() {
                SetWindowPos(hwnd, 0, x, y, 0, 0, flags | SWP_NOZORDER);
                return;
            } else {
                HWND_TOPMOST
            };
            SetWindowPos(hwnd, insert, x, y, 0, 0, flags);
        }
    }

    struct Drawn {
        temp: Vec<u16>,
        label: Vec<u16>,
        temp_rc: Rect,
        label_rc: Rect,
        color: u32,
        x0: i32,
        x1: i32,
    }

    fn paint(hwnd: isize, frame: &Frame, layout: &Layout) -> bool {
        let Some(dib) = Dib::new(frame.w, frame.h) else {
            return false;
        };
        let temp_font = create_font(layout.temp_px, frame.dpi, FW_SEMIBOLD);
        let label_font = create_font(layout.label_px, frame.dpi, FW_NORMAL);
        let drawn = drawn_columns(frame, layout);
        let themed = draw_theme_columns(hwnd, dib.dc, temp_font, label_font, &drawn)
            && has_alpha(dib.bits, dib.count);
        if !themed {
            dib.clear();
            unsafe {
                SetBkMode(dib.dc, TRANSPARENT);
                SetTextColor(dib.dc, COLOR_LIGHT);
            }
            for col in &drawn {
                draw_gdi_line(dib.dc, temp_font, &col.temp, col.temp_rc);
                draw_gdi_line(dib.dc, label_font, &col.label, col.label_rc);
            }
        }
        for col in &drawn {
            tint_span(dib.bits, frame.w, frame.h, col.x0, col.x1, col.color);
        }
        let ok = present_window(hwnd, dib.dc, frame.x, frame.y, frame.w, frame.h);
        delete_font(dib.dc, temp_font);
        delete_font(dib.dc, label_font);
        ok
    }

    fn drawn_columns(frame: &Frame, layout: &Layout) -> Vec<Drawn> {
        let mut x = 0;
        let mut out = Vec::with_capacity(frame.cols.len());
        for (i, col) in frame.cols.iter().enumerate() {
            let width = layout.widths.get(i).copied().unwrap_or(0);
            let (temp_rc, label_rc) = line_rects(layout, x, width, frame.h);
            out.push(Drawn {
                temp: wide(&col.text),
                label: wide(&col.label),
                temp_rc,
                label_rc,
                color: col.color,
                x0: x,
                x1: x + width,
            });
            x += width;
            if i + 1 != frame.cols.len() {
                x += layout.col_gap;
            }
        }
        out
    }

    fn line_rects(layout: &Layout, left: i32, width: i32, height: i32) -> (Rect, Rect) {
        let stack = layout
            .temp_h
            .saturating_add(layout.gap_y)
            .saturating_add(layout.label_h);
        let top = ((height - stack) / 2).max(0);
        let temp = Rect {
            left,
            top,
            right: left + width,
            bottom: top + layout.temp_h,
        };
        let label = Rect {
            left,
            top: temp.bottom + layout.gap_y,
            right: left + width,
            bottom: temp.bottom + layout.gap_y + layout.label_h,
        };
        (temp, label)
    }

    fn draw_theme_columns(
        hwnd: isize,
        dc: isize,
        temp_font: isize,
        label_font: isize,
        drawn: &[Drawn],
    ) -> bool {
        let Some(api) = theme_api() else {
            return false;
        };
        if temp_font == 0 || label_font == 0 || drawn.is_empty() {
            return false;
        }
        unsafe {
            let class = wide("TASKBAND");
            let theme = (api.open)(hwnd, class.as_ptr());
            if theme == 0 {
                return false;
            }
            SetBkMode(dc, TRANSPARENT);
            let mut ok = true;
            for col in drawn {
                let opts = dtt_opts(col.color);
                ok &= draw_theme_line(api, theme, dc, temp_font, &col.temp, col.temp_rc, &opts);
                ok &= draw_theme_line(api, theme, dc, label_font, &col.label, col.label_rc, &opts);
            }
            (api.close)(theme);
            ok
        }
    }

    fn draw_theme_line(
        api: &ThemeApi,
        theme: isize,
        dc: isize,
        font: isize,
        text: &[u16],
        rect: Rect,
        opts: &DttOpts,
    ) -> bool {
        unsafe {
            let old = SelectObject(dc, font);
            let mut rect = rect;
            let status = (api.draw)(theme, dc, 0, 0, text.as_ptr(), -1, DT_DRAW, &mut rect, opts);
            if old != 0 {
                SelectObject(dc, old);
            }
            status == 0
        }
    }

    fn draw_gdi_line(dc: isize, font: isize, text: &[u16], rect: Rect) {
        if font == 0 {
            return;
        }
        unsafe {
            let old = SelectObject(dc, font);
            let mut rect = rect;
            DrawTextW(dc, text.as_ptr(), -1, &mut rect, DT_DRAW);
            if old != 0 {
                SelectObject(dc, old);
            }
        }
    }

    fn has_alpha(bits: *mut u8, count: usize) -> bool {
        pixels(bits, count).iter().any(|px| px >> 24 != 0)
    }

    fn tint_span(bits: *mut u8, width: i32, height: i32, x0: i32, x1: i32, color: u32) {
        if width <= 0 || height <= 0 {
            return;
        }
        let stride = width as usize;
        let count = stride.saturating_mul(height as usize);
        let px = pixels(bits, count);
        let from_alpha = px.iter().any(|pixel| pixel >> 24 != 0);
        let tr = color & 0xff;
        let tg = (color >> 8) & 0xff;
        let tb = (color >> 16) & 0xff;
        let x0 = x0.max(0) as usize;
        let x1 = (x1.max(0) as usize).min(stride);
        if x0 >= x1 {
            return;
        }
        for y in 0..height as usize {
            for x in x0..x1 {
                let pixel = &mut px[y * stride + x];
                let value = *pixel;
                let coverage = if from_alpha {
                    value >> 24
                } else {
                    let b = value & 0xff;
                    let g = (value >> 8) & 0xff;
                    let r = (value >> 16) & 0xff;
                    r.max(g).max(b)
                };
                if coverage == 0 {
                    *pixel = 0;
                    continue;
                }
                let r = tr * coverage / 255;
                let g = tg * coverage / 255;
                let b = tb * coverage / 255;
                *pixel = (coverage << 24) | (r << 16) | (g << 8) | b;
            }
        }
    }

    fn pixels(bits: *mut u8, count: usize) -> &'static mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(bits as *mut u32, count) }
    }

    fn dtt_opts(color: u32) -> DttOpts {
        DttOpts {
            size: std::mem::size_of::<DttOpts>() as u32,
            flags: DTT_TEXTCOLOR | DTT_COMPOSITED,
            text_color: color,
            border: 0,
            shadow: 0,
            shadow_type: 0,
            shadow_x: 0,
            shadow_y: 0,
            border_size: 0,
            font_prop: 0,
            color_prop: 0,
            state_id: 0,
            apply_overlay: 0,
            glow: 0,
            callback: std::ptr::null(),
            param: 0,
        }
    }

    struct ThemeApi {
        open: OpenTheme,
        close: CloseTheme,
        draw: DrawTheme,
    }

    type OpenTheme = unsafe extern "system" fn(hwnd: isize, class: *const u16) -> isize;
    type CloseTheme = unsafe extern "system" fn(theme: isize) -> i32;
    type DrawTheme = unsafe extern "system" fn(
        theme: isize,
        hdc: isize,
        part: i32,
        state: i32,
        text: *const u16,
        cch: i32,
        flags: u32,
        rect: *mut Rect,
        opts: *const DttOpts,
    ) -> i32;

    fn theme_api() -> Option<&'static ThemeApi> {
        static API: OnceLock<Option<ThemeApi>> = OnceLock::new();
        API.get_or_init(|| unsafe {
            let name = wide("uxtheme.dll");
            let lib = LoadLibraryW(name.as_ptr());
            if lib == 0 {
                return None;
            }
            let open = GetProcAddress(lib, b"OpenThemeData\0".as_ptr());
            let close = GetProcAddress(lib, b"CloseThemeData\0".as_ptr());
            let draw = GetProcAddress(lib, b"DrawThemeTextEx\0".as_ptr());
            if open.is_null() || close.is_null() || draw.is_null() {
                return None;
            }
            Some(ThemeApi {
                open: std::mem::transmute(open),
                close: std::mem::transmute(close),
                draw: std::mem::transmute(draw),
            })
        })
        .as_ref()
    }

    fn present_window(hwnd: isize, dc: isize, x: i32, y: i32, w: i32, h: i32) -> bool {
        if present(hwnd, dc, x, y, w, h) {
            return true;
        }
        unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        if present(hwnd, dc, x, y, w, h) {
            return true;
        }
        ALLOW_HIDE.store(true, Ordering::SeqCst);
        unsafe { ShowWindow(hwnd, SW_HIDE) };
        ALLOW_HIDE.store(false, Ordering::SeqCst);
        false
    }

    fn present(hwnd: isize, dc: isize, x: i32, y: i32, w: i32, h: i32) -> bool {
        unsafe {
            let dst = Point { x, y };
            let size = WinSize { cx: w, cy: h };
            let src = Point { x: 0, y: 0 };
            let blend = Blend {
                op: AC_SRC_OVER,
                flags: 0,
                alpha: 255,
                format: AC_SRC_ALPHA,
            };
            UpdateLayeredWindow(hwnd, 0, &dst, &size, dc, &src, 0, &blend, ULW_ALPHA) != 0
        }
    }

    struct Dib {
        dc: isize,
        bitmap: isize,
        old: isize,
        bits: *mut u8,
        count: usize,
    }

    impl Dib {
        fn new(w: i32, h: i32) -> Option<Self> {
            if !(1..=2000).contains(&w) || !(1..=400).contains(&h) {
                return None;
            }
            unsafe {
                let dc = CreateCompatibleDC(0);
                if dc == 0 {
                    return None;
                }
                let header = BitmapInfoHeader {
                    size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                    width: w,
                    height: -h,
                    planes: 1,
                    bit_count: 32,
                    compression: 0,
                    size_image: 0,
                    x_pels: 0,
                    y_pels: 0,
                    clr_used: 0,
                    clr_important: 0,
                };
                let mut bits: *mut u8 = std::ptr::null_mut();
                let bitmap = CreateDIBSection(dc, &header, 0, &mut bits, 0, 0);
                if bitmap == 0 || bits.is_null() {
                    if bitmap != 0 {
                        DeleteObject(bitmap);
                    }
                    DeleteDC(dc);
                    return None;
                }
                let old = SelectObject(dc, bitmap);
                let count = (w as usize).saturating_mul(h as usize);
                std::ptr::write_bytes(bits, 0, count.saturating_mul(4));
                Some(Self {
                    dc,
                    bitmap,
                    old,
                    bits,
                    count,
                })
            }
        }

        fn clear(&self) {
            unsafe { std::ptr::write_bytes(self.bits, 0, self.count.saturating_mul(4)) };
        }
    }

    impl Drop for Dib {
        fn drop(&mut self) {
            unsafe {
                if self.dc != 0 && self.old != 0 {
                    SelectObject(self.dc, self.old);
                }
                if self.bitmap != 0 {
                    DeleteObject(self.bitmap);
                }
                if self.dc != 0 {
                    DeleteDC(self.dc);
                }
            }
        }
    }

    fn layout_columns(cols: &[Col], dpi: i32, tray_h: i32) -> Layout {
        let (temp_px, label_px, temp_h, label_h, gap_y) = fit_fonts(dpi, tray_h);
        let col_gap = css_device(8, dpi);
        let dc = unsafe { CreateCompatibleDC(0) };
        let temp_font = create_font(temp_px, dpi, FW_SEMIBOLD);
        let label_font = create_font(label_px, dpi, FW_NORMAL);
        let mut widths = Vec::with_capacity(cols.len());
        for col in cols {
            let (temp_w, _) = text_extent(dc, temp_font, &col.text);
            let (label_w, _) = text_extent(dc, label_font, &col.label);
            widths.push(temp_w.max(label_w).saturating_add(4).clamp(8, 800));
        }
        delete_font(dc, temp_font);
        delete_font(dc, label_font);
        if dc != 0 {
            unsafe { DeleteDC(dc) };
        }
        let gaps = col_gap.saturating_mul(cols.len().saturating_sub(1) as i32);
        let w = widths
            .iter()
            .copied()
            .sum::<i32>()
            .saturating_add(gaps)
            .clamp(8, 1600);
        Layout {
            widths,
            col_gap,
            w,
            temp_px,
            label_px,
            temp_h,
            label_h,
            gap_y,
        }
    }

    fn fit_fonts(dpi: i32, tray_h: i32) -> (i32, i32, i32, i32, i32) {
        let (mut temp_px, mut label_px) = css_sizes(dpi, tray_h);
        loop {
            let temp_h = line_box(temp_px, dpi);
            let label_h = line_box(label_px, dpi);
            let gap_y = css_device(1, dpi);
            if temp_h + gap_y + label_h <= tray_h || temp_px <= 11 {
                return (temp_px, label_px, temp_h, label_h, gap_y);
            }
            temp_px -= 1;
            label_px = (temp_px * 11 / 15).max(8);
        }
    }

    fn css_sizes(dpi: i32, tray_h: i32) -> (i32, i32) {
        let logical = mul_div(tray_h, 96, dpi.max(96));
        if logical < 42 {
            (13, 10)
        } else {
            (15, 11)
        }
    }

    fn css_device(px: i32, dpi: i32) -> i32 {
        mul_div(px.max(1), dpi.max(96), 96).max(1)
    }

    fn line_box(css_px: i32, dpi: i32) -> i32 {
        mul_div(css_px.saturating_mul(21), dpi.max(96), 1920).max(1)
    }

    fn text_extent(dc: isize, font: isize, text: &str) -> (i32, i32) {
        if dc == 0 || font == 0 {
            return (48, 16);
        }
        let text = wide(text);
        unsafe {
            let old = SelectObject(dc, font);
            let mut rc = Rect::zero();
            DrawTextW(dc, text.as_ptr(), -1, &mut rc, DT_MEASURE);
            if old != 0 {
                SelectObject(dc, old);
            }
            ((rc.right - rc.left).max(1), (rc.bottom - rc.top).max(1))
        }
    }

    fn create_font(css_px: i32, dpi: i32, weight: i32) -> isize {
        // "Segoe UI" at 600 is snapped to Bold (700) by GDI, which pinches the digit 8.
        // The previous taskbar used CSS font-weight 600, i.e. Segoe UI Semibold.
        let (face, weight) = if weight >= FW_SEMIBOLD {
            match semibold_face() {
                Some(face) => (face, FW_SEMIBOLD),
                None => (preferred_face(), FW_NORMAL),
            }
        } else {
            (preferred_face(), weight)
        };
        let height = -css_device(css_px, dpi);
        unsafe {
            let font = CreateFontW(
                height,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                face.charset,
                0,
                0,
                ANTIALIASED_QUALITY,
                0,
                face.name.as_ptr(),
            );
            if font != 0 {
                return font;
            }
            let tahoma = wide("Tahoma");
            CreateFontW(
                height,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                0,
                0,
                ANTIALIASED_QUALITY,
                0,
                tahoma.as_ptr(),
            )
        }
    }

    fn delete_font(dc: isize, font: isize) {
        if font == 0 {
            return;
        }
        unsafe {
            if dc != 0 {
                let stock = GetStockObject(SYSTEM_FONT);
                if stock != 0 {
                    SelectObject(dc, stock);
                }
            }
            DeleteObject(font);
        }
    }

    struct FaceChoice {
        name: Vec<u16>,
        charset: u32,
    }

    fn semibold_face() -> Option<&'static FaceChoice> {
        static FACE: OnceLock<Option<FaceChoice>> = OnceLock::new();
        FACE.get_or_init(|| {
            if face_resolves("Segoe UI Semibold", DEFAULT_CHARSET, "semibold") {
                Some(FaceChoice {
                    name: wide("Segoe UI Semibold"),
                    charset: DEFAULT_CHARSET,
                })
            } else {
                None
            }
        })
        .as_ref()
    }

    fn preferred_face() -> &'static FaceChoice {
        static FACE: OnceLock<FaceChoice> = OnceLock::new();
        FACE.get_or_init(|| {
            if face_resolves("Segoe UI", DEFAULT_CHARSET, "segoe") {
                FaceChoice {
                    name: wide("Segoe UI"),
                    charset: DEFAULT_CHARSET,
                }
            } else if face_resolves("Microsoft YaHei UI", GB2312_CHARSET, "yahei")
                || face_resolves("微软雅黑 UI", GB2312_CHARSET, "雅黑")
            {
                FaceChoice {
                    name: wide("Microsoft YaHei UI"),
                    charset: GB2312_CHARSET,
                }
            } else if face_resolves("微软雅黑", GB2312_CHARSET, "雅黑") {
                FaceChoice {
                    name: wide("微软雅黑"),
                    charset: GB2312_CHARSET,
                }
            } else {
                FaceChoice {
                    name: wide("Tahoma"),
                    charset: DEFAULT_CHARSET,
                }
            }
        })
    }

    fn face_resolves(name: &str, charset: u32, hint: &str) -> bool {
        unsafe {
            let requested = wide(name);
            let font = CreateFontW(
                -16,
                0,
                0,
                0,
                FW_NORMAL,
                0,
                0,
                0,
                charset,
                0,
                0,
                ANTIALIASED_QUALITY,
                0,
                requested.as_ptr(),
            );
            if font == 0 {
                return false;
            }
            let dc = CreateCompatibleDC(0);
            if dc == 0 {
                DeleteObject(font);
                return false;
            }
            let old = SelectObject(dc, font);
            let mut buf = [0u16; 64];
            let n = GetTextFaceW(dc, buf.len() as i32, buf.as_mut_ptr());
            if old != 0 {
                SelectObject(dc, old);
            }
            DeleteObject(font);
            DeleteDC(dc);
            if n <= 0 {
                return false;
            }
            let end = buf.iter().position(|&unit| unit == 0).unwrap_or(n as usize);
            let face = String::from_utf16_lossy(&buf[..end.min(buf.len())]);
            face.to_ascii_lowercase().contains(hint) || face.contains(hint)
        }
    }

    fn mm_to_px(dpi: i32, mm: i32) -> i32 {
        mul_div(dpi.max(96), mm.saturating_mul(10), 254).max(1)
    }

    fn mul_div(value: i32, num: i32, den: i32) -> i32 {
        if den == 0 {
            return value;
        }
        let product = value as i64 * num as i64;
        let den = den as i64;
        let rounded = if product >= 0 {
            product + den / 2
        } else {
            product - den / 2
        };
        (rounded / den) as i32
    }

    fn system_uses_light_theme() -> Option<bool> {
        unsafe {
            let sub = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
            let mut key = 0isize;
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                sub.as_ptr(),
                0,
                KEY_QUERY_VALUE,
                &mut key,
            ) != 0
            {
                return None;
            }
            let name = wide("SystemUsesLightTheme");
            let mut data = 0u32;
            let mut size = 4u32;
            let mut kind = 0u32;
            let status = RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                &mut data as *mut u32 as *mut u8,
                &mut size,
            );
            RegCloseKey(key);
            if status != 0 || kind != REG_DWORD {
                None
            } else {
                Some(data != 0)
            }
        }
    }

    struct Spot {
        tray_left: i32,
        y: i32,
        h: i32,
        dpi: i32,
    }

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
            Some(Spot {
                tray_left,
                y: bar.top,
                h: bar.bottom - bar.top,
                dpi: dpi_for(taskbar),
            })
        }
    }

    fn dpi_for(hwnd: isize) -> i32 {
        unsafe {
            if let Some(get_dpi) = dpi_fn() {
                let dpi = get_dpi(hwnd);
                if dpi >= 96 {
                    return dpi as i32;
                }
            }
            let dc = GetDC(0);
            if dc == 0 {
                return 96;
            }
            let dpi = GetDeviceCaps(dc, LOGPIXELSY);
            ReleaseDC(0, dc);
            if dpi >= 96 {
                dpi
            } else {
                96
            }
        }
    }

    fn dpi_fn() -> Option<unsafe extern "system" fn(isize) -> u32> {
        static FN: OnceLock<Option<unsafe extern "system" fn(isize) -> u32>> = OnceLock::new();
        *FN.get_or_init(|| unsafe {
            let module = GetModuleHandleW(wide("user32.dll").as_ptr());
            let ptr = GetProcAddress(module, b"GetDpiForWindow\0".as_ptr());
            if ptr.is_null() {
                None
            } else {
                Some(std::mem::transmute(ptr))
            }
        })
    }

    fn is_bottom_bar(bar: &Rect, monitor: &Rect) -> bool {
        let w = bar.right - bar.left;
        let h = bar.bottom - bar.top;
        w > h.saturating_mul(2)
            && (24..=180).contains(&h)
            && bar.bottom >= monitor.bottom - 6
            && bar.top < monitor.bottom - 16
    }

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

    fn keep_visible(hwnd: isize) {
        unsafe {
            let taskbar = FindWindowW(wide("Shell_TrayWnd").as_ptr(), std::ptr::null());
            if taskbar != 0 && GetWindowLongPtrW(hwnd, GWLP_HWNDPARENT) != taskbar {
                SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, taskbar);
            }
            let mut cloaked = 0u32;
            if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked, 4) == 0 && cloaked != 0 {
                let shown = 0u32;
                DwmSetWindowAttribute(hwnd, DWMWA_CLOAK, &shown, 4);
            }
            if IsIconic(hwnd) != 0 {
                let mut place = WindowPlacement {
                    length: std::mem::size_of::<WindowPlacement>() as u32,
                    flags: 0,
                    show_cmd: SW_SHOWNOACTIVATE as u32,
                    min_position: Point { x: 0, y: 0 },
                    max_position: Point { x: 0, y: 0 },
                    normal_position: Rect::zero(),
                };
                if GetWindowPlacement(hwnd, &mut place) != 0 {
                    place.length = std::mem::size_of::<WindowPlacement>() as u32;
                    place.show_cmd = SW_SHOWNOACTIVATE as u32;
                    SetWindowPlacement(hwnd, &place);
                }
            } else if IsWindowVisible(hwnd) == 0 {
                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            lift_above_taskbar(hwnd);
        }
    }

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

    fn show_desktop_active() -> bool {
        matches!(
            class_name(unsafe { GetForegroundWindow() }).as_str(),
            "Progman" | "WorkerW"
        )
    }

    fn menu_is_open() -> bool {
        let class = class_name(unsafe { GetForegroundWindow() });
        class == "#32768" || class == "NotifyIconOverflowWindow"
    }

    fn class_name(hwnd: isize) -> String {
        let mut buf = [0u16; 256];
        let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if n <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..n as usize])
    }

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

    fn create_window(x: i32, y: i32, w: i32, h: i32) -> isize {
        if !ensure_class() {
            return 0;
        }
        unsafe {
            let class = wide(CLASS_NAME);
            let title = wide("");
            let tray_class = wide("Shell_TrayWnd");
            let taskbar = FindWindowW(tray_class.as_ptr(), std::ptr::null());
            let ex = WS_EX_LAYERED
                | WS_EX_TOOLWINDOW
                | WS_EX_TOPMOST
                | WS_EX_TRANSPARENT
                | WS_EX_NOACTIVATE;
            let style = WS_POPUP | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
            let hwnd = CreateWindowExW(
                ex,
                class.as_ptr(),
                title.as_ptr(),
                style,
                x,
                y,
                w,
                h,
                taskbar,
                0,
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            );
            if hwnd != 0 && taskbar != 0 {
                SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, taskbar);
            }
            hwnd
        }
    }

    fn ensure_class() -> bool {
        static READY: AtomicBool = AtomicBool::new(false);
        if READY.load(Ordering::SeqCst) {
            return true;
        }
        if register_class() {
            READY.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    fn register_class() -> bool {
        unsafe {
            let class = wide(CLASS_NAME);
            let wc = WndClassEx {
                size: std::mem::size_of::<WndClassEx>() as u32,
                style: 0,
                wnd_proc: wnd_proc as *const () as isize,
                cls_extra: 0,
                wnd_extra: 0,
                instance: GetModuleHandleW(std::ptr::null()),
                icon: 0,
                cursor: 0,
                background: 0,
                menu: std::ptr::null(),
                class: class.as_ptr(),
                icon_sm: 0,
            };
            if RegisterClassExW(&wc) != 0 {
                return true;
            }
            GetLastError() == ERROR_CLASS_ALREADY_EXISTS
        }
    }

    fn drain_messages() {
        unsafe {
            let mut msg = MsgBuf { bytes: [0; 64] };
            let mut n = 0;
            while n < 64 && PeekMessageW(std::ptr::from_mut(&mut msg).cast(), 0, 0, 0, 1) != 0 {
                TranslateMessage(std::ptr::from_ref(&msg).cast());
                DispatchMessageW(std::ptr::from_ref(&msg).cast());
                n += 1;
            }
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: isize,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        if !ALLOW_HIDE.load(Ordering::SeqCst) {
            match msg {
                WM_WINDOWPOSCHANGING if lparam != 0 => {
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
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    #[repr(C, align(16))]
    struct MsgBuf {
        bytes: [u8; 64],
    }

    #[repr(C)]
    struct WndClassEx {
        size: u32,
        style: u32,
        wnd_proc: isize,
        cls_extra: i32,
        wnd_extra: i32,
        instance: isize,
        icon: isize,
        cursor: isize,
        background: isize,
        menu: *const u16,
        class: *const u16,
        icon_sm: isize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

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

    #[repr(C)]
    struct MonitorInfo {
        cb_size: u32,
        rc_monitor: Rect,
        rc_work: Rect,
        flags: u32,
    }

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct WinSize {
        cx: i32,
        cy: i32,
    }

    #[repr(C)]
    struct Blend {
        op: u8,
        flags: u8,
        alpha: u8,
        format: u8,
    }

    #[repr(C)]
    struct WindowPlacement {
        length: u32,
        flags: u32,
        show_cmd: u32,
        min_position: Point,
        max_position: Point,
        normal_position: Rect,
    }

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

    #[repr(C)]
    struct BitmapInfoHeader {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bit_count: u16,
        compression: u32,
        size_image: u32,
        x_pels: i32,
        y_pels: i32,
        clr_used: u32,
        clr_important: u32,
    }

    #[repr(C)]
    struct DttOpts {
        size: u32,
        flags: u32,
        text_color: u32,
        border: u32,
        shadow: u32,
        shadow_type: i32,
        shadow_x: i32,
        shadow_y: i32,
        border_size: i32,
        font_prop: i32,
        color_prop: i32,
        state_id: i32,
        apply_overlay: i32,
        glow: i32,
        callback: *const (),
        param: isize,
    }

    #[cfg(target_pointer_width = "64")]
    const _: () = {
        assert!(std::mem::size_of::<DttOpts>() == 72);
        assert!(std::mem::size_of::<WndClassEx>() == 80);
        assert!(std::mem::size_of::<BitmapInfoHeader>() == 40);
    };

    #[link(name = "user32")]
    extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> isize;
        fn FindWindowExW(
            parent: isize,
            after: isize,
            class: *const u16,
            title: *const u16,
        ) -> isize;
        fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
        fn GetForegroundWindow() -> isize;
        fn GetClassNameW(hwnd: isize, class: *mut u16, max: i32) -> i32;
        fn IsWindowVisible(hwnd: isize) -> i32;
        fn IsIconic(hwnd: isize) -> i32;
        fn IsWindow(hwnd: isize) -> i32;
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
        fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn RegisterClassExW(class: *const WndClassEx) -> u16;
        fn CreateWindowExW(
            ex: u32,
            class: *const u16,
            title: *const u16,
            style: u32,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            parent: isize,
            menu: isize,
            instance: isize,
            param: *const u8,
        ) -> isize;
        fn DestroyWindow(hwnd: isize) -> i32;
        fn PeekMessageW(msg: *mut u8, hwnd: isize, min: u32, max: u32, remove: u32) -> i32;
        fn TranslateMessage(msg: *const u8) -> i32;
        fn DispatchMessageW(msg: *const u8) -> isize;
        fn DrawTextW(hdc: isize, text: *const u16, cch: i32, rect: *mut Rect, format: u32) -> i32;
        fn UpdateLayeredWindow(
            hwnd: isize,
            dst_dc: isize,
            dst: *const Point,
            size: *const WinSize,
            src_dc: isize,
            src: *const Point,
            key: u32,
            blend: *const Blend,
            flags: u32,
        ) -> i32;
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn CreateCompatibleDC(hdc: isize) -> isize;
        fn DeleteDC(hdc: isize) -> i32;
        fn CreateDIBSection(
            hdc: isize,
            info: *const BitmapInfoHeader,
            usage: u32,
            bits: *mut *mut u8,
            section: isize,
            offset: u32,
        ) -> isize;
        fn SelectObject(hdc: isize, object: isize) -> isize;
        fn DeleteObject(object: isize) -> i32;
        fn CreateFontW(
            height: i32,
            width: i32,
            escapement: i32,
            orientation: i32,
            weight: i32,
            italic: u32,
            underline: u32,
            strike: u32,
            charset: u32,
            out_precision: u32,
            clip_precision: u32,
            quality: u32,
            pitch: u32,
            face: *const u16,
        ) -> isize;
        fn GetTextFaceW(hdc: isize, cch: i32, face: *mut u16) -> i32;
        fn SetBkMode(hdc: isize, mode: i32) -> i32;
        fn SetTextColor(hdc: isize, color: u32) -> u32;
        fn GetDeviceCaps(hdc: isize, index: i32) -> i32;
        fn GetStockObject(object: i32) -> isize;
    }

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmGetWindowAttribute(hwnd: isize, attr: u32, value: *mut u32, size: u32) -> i32;
        fn DwmSetWindowAttribute(hwnd: isize, attr: u32, value: *const u32, size: u32) -> i32;
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            key: isize,
            sub: *const u16,
            options: u32,
            sam: u32,
            result: *mut isize,
        ) -> i32;
        fn RegQueryValueExW(
            key: isize,
            name: *const u16,
            reserved: *mut u32,
            kind: *mut u32,
            data: *mut u8,
            size: *mut u32,
        ) -> i32;
        fn RegCloseKey(key: isize) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
        fn LoadLibraryW(name: *const u16) -> isize;
        fn GetProcAddress(module: isize, name: *const u8) -> *mut ();
        fn GetLastError() -> u32;
    }
}
