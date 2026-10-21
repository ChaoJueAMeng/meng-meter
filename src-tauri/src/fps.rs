use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct Bridge {
    counts: Mutex<HashMap<u32, u64>>,
    running: AtomicBool,
}

struct Track {
    pid: u32,
    count: u64,
    at: Instant,
}

struct Memory {
    last_pid: u32,
    track: Option<Track>,
    last_fps: Option<f64>,
}

struct RefreshCache {
    at: Instant,
    hz: Option<f64>,
}

pub struct Meter {
    bridge: Arc<Bridge>,
    memory: Arc<Mutex<Memory>>,
    refresh: Mutex<RefreshCache>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Meter {
    pub fn start() -> Self {
        let bridge = Arc::new(Bridge {
            counts: Mutex::new(HashMap::new()),
            running: AtomicBool::new(false),
        });
        let memory = Arc::new(Mutex::new(Memory {
            last_pid: 0,
            track: None,
            last_fps: None,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let raw = Arc::as_ptr(&bridge) as usize;
        let held = Arc::clone(&bridge);
        let etw = thread::spawn(move || {
            let _held = held;
            unsafe { fps_run(on_ready, on_dirty, raw as *mut c_void) };
        });
        let bridge_s = Arc::clone(&bridge);
        let memory_s = Arc::clone(&memory);
        let stop_s = Arc::clone(&stop);
        let sampler = thread::spawn(move || {
            while !stop_s.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(200));
                sample(&bridge_s, &memory_s);
            }
        });
        Self {
            bridge,
            memory,
            refresh: Mutex::new(RefreshCache {
                at: Instant::now() - Duration::from_secs(60),
                hz: None,
            }),
            stop,
            threads: vec![etw, sampler],
        }
    }

    pub fn frames_per_sec(&self) -> Option<f64> {
        if !self.bridge.running.load(Ordering::Acquire) {
            return None;
        }
        self.memory.lock().ok().and_then(|memory| memory.last_fps)
    }

    pub fn refresh_hz(&self) -> Option<f64> {
        let mut cache = self.refresh.lock().ok()?;
        if cache.at.elapsed() < Duration::from_secs(5) {
            return cache.hz;
        }
        cache.at = Instant::now();
        cache.hz = read_refresh();
        cache.hz
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        unsafe { fps_stop() };
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn sample(bridge: &Bridge, memory: &Mutex<Memory>) {
    if !bridge.running.load(Ordering::Acquire) {
        return;
    }
    let Ok(mut memory) = memory.lock() else {
        return;
    };
    let Some(pid) = foreground_pid(&mut memory) else {
        return;
    };
    let count = {
        let Ok(counts) = bridge.counts.lock() else {
            return;
        };
        counts.get(&pid).copied().unwrap_or(0)
    };
    let now = Instant::now();
    let Some(prev) = memory.track.as_ref() else {
        memory.track = Some(Track {
            pid,
            count,
            at: now,
        });
        return;
    };
    if prev.pid != pid {
        memory.track = Some(Track {
            pid,
            count,
            at: now,
        });
        return;
    }
    let dt = now.duration_since(prev.at).as_secs_f64();
    let delta = count.saturating_sub(prev.count);
    // 实时 ETW 要等缓冲区刷出才看得到帧。空档不是 0 帧，窗口继续开着。
    if delta == 0 {
        if dt < 1.5 {
            return;
        }
        memory.track = Some(Track {
            pid,
            count,
            at: now,
        });
        memory.last_fps = Some(0.0);
        return;
    }
    if dt < 0.45 {
        return;
    }
    memory.track = Some(Track {
        pid,
        count,
        at: now,
    });
    let fps = delta as f64 / dt;
    if fps > 1000.0 {
        return;
    }
    memory.last_fps = Some(fps);
}

extern "C" fn on_ready(user: *mut c_void) {
    if user.is_null() {
        return;
    }
    let bridge = unsafe { &*(user as *const Bridge) };
    bridge.running.store(true, Ordering::Release);
}

extern "C" fn on_dirty(pid: u32, user: *mut c_void) {
    if user.is_null() || pid == 0 {
        return;
    }
    let bridge = unsafe { &*(user as *const Bridge) };
    let Ok(mut counts) = bridge.counts.lock() else {
        return;
    };
    *counts.entry(pid).or_insert(0) += 1;
}

fn foreground_pid(memory: &mut Memory) -> Option<u32> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd == 0 {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 {
        return None;
    }
    if unsafe { fps_is_self_tree(pid) } != 0 {
        return (memory.last_pid != 0).then_some(memory.last_pid);
    }
    memory.last_pid = pid;
    Some(pid)
}

fn read_refresh() -> Option<f64> {
    let mut info = [0u8; 292];
    info[..4].copy_from_slice(&292u32.to_ne_bytes());
    let hr = unsafe { DwmGetCompositionTimingInfo(0, info.as_mut_ptr()) };
    if hr < 0 {
        return None;
    }
    let num = u32::from_ne_bytes(info[4..8].try_into().ok()?);
    let den = u32::from_ne_bytes(info[8..12].try_into().ok()?);
    if den == 0 {
        return None;
    }
    let hz = num as f64 / den as f64;
    (30.0..=1000.0).contains(&hz).then_some(hz)
}

#[link(name = "user32")]
extern "system" {
    fn GetForegroundWindow() -> isize;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
}

#[link(name = "dwmapi")]
extern "system" {
    fn DwmGetCompositionTimingInfo(hwnd: isize, info: *mut u8) -> i32;
}

extern "C" {
    fn fps_run(
        on_ready: extern "C" fn(*mut c_void),
        on_dirty: extern "C" fn(u32, *mut c_void),
        user: *mut c_void,
    ) -> i32;
    fn fps_stop();
    fn fps_is_self_tree(pid: u32) -> i32;
}
