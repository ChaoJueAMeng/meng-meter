use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::Networks;
use tauri::{AppHandle, Manager};

const INTERVAL: Duration = Duration::from_secs(60);
const HEADER: &str = "时间,内存占用%,上行B/s,下行B/s,CPU温度,显卡温度\r\n";

struct Stamp {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

pub(crate) fn spawn(app: AppHandle) {
    thread::spawn(move || {
        let mut nets = Networks::new_with_refreshed_list();
        let mut last = Instant::now();
        let _ = prune_current(&app);
        loop {
            thread::sleep(INTERVAL);
            let dt = last.elapsed().as_secs_f64().max(0.05);
            nets.refresh(true);
            last = Instant::now();
            let (enabled, _) = log_prefs(&app);
            if enabled {
                let (up, down) = rates(&nets, dt);
                if let Some(reading) = super::host_reading(&app) {
                    let _ = append(&app, &reading, up, down);
                }
            }
            let _ = prune_current(&app);
        }
    });
}

pub(crate) fn prune(app: &AppHandle, keep_days: u16) -> Result<(), String> {
    let dir = log_dir(app)?;
    let stamp = local_stamp();
    let today = days_from_civil(stamp.year, stamp.month, stamp.day).ok_or("无法读取本地日期")?;
    for entry in fs::read_dir(&dir).map_err(|err| err.to_string())?.flatten() {
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(day) = parse_log_day(&name) else {
            continue;
        };
        if is_expired(today, day, keep_days) {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

pub(crate) fn open_dir(app: &AppHandle) -> Result<(), String> {
    reveal(&log_dir(app)?)
}

fn prune_current(app: &AppHandle) -> Result<(), String> {
    let (_, keep_days) = log_prefs(app);
    prune(app, keep_days)
}

fn log_prefs(app: &AppHandle) -> (bool, u16) {
    app.state::<super::MeterState>()
        .settings
        .lock()
        .map(|settings| (settings.log_enabled, settings.log_keep_days))
        .unwrap_or((true, 7))
}

fn log_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_log_dir().map_err(|err| err.to_string())?;
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    Ok(dir)
}

fn rates(nets: &Networks, dt: f64) -> (f64, f64) {
    let mut down = 0u64;
    let mut up = 0u64;
    for (name, data) in nets.iter() {
        if super::skip_iface(name) {
            continue;
        }
        down += data.received();
        up += data.transmitted();
    }
    let dt = dt.max(0.05);
    (up as f64 / dt, down as f64 / dt)
}

fn append(app: &AppHandle, reading: &super::HostReading, up: f64, down: f64) -> Result<(), String> {
    let stamp = local_stamp();
    let path = log_dir(app)?.join(format!(
        "{:04}-{:02}-{:02}.csv",
        stamp.year, stamp.month, stamp.day
    ));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| err.to_string())?;
    if file.metadata().map(|meta| meta.len() == 0).unwrap_or(true) {
        file.write_all("\u{feff}".as_bytes())
            .map_err(|err| err.to_string())?;
        file.write_all(HEADER.as_bytes())
            .map_err(|err| err.to_string())?;
    }
    let line = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02},{},{},{},{},{}\r\n",
        stamp.year,
        stamp.month,
        stamp.day,
        stamp.hour,
        stamp.minute,
        stamp.second,
        fmt_num(reading.mem_percent),
        fmt_bps(up),
        fmt_bps(down),
        fmt_opt(reading.cpu_c),
        fmt_opt(reading.gpu_c),
    );
    file.write_all(line.as_bytes())
        .map_err(|err| err.to_string())?;
    file.flush().map_err(|err| err.to_string())
}

fn fmt_num(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.1}")
    } else {
        String::new()
    }
}

fn fmt_opt(value: Option<f64>) -> String {
    value
        .filter(|item| item.is_finite())
        .map(|item| format!("{item:.1}"))
        .unwrap_or_default()
}

fn fmt_bps(value: f64) -> String {
    if !value.is_finite() || value <= 0.0 {
        return "0".into();
    }
    let rounded = value.round();
    if rounded >= u64::MAX as f64 {
        u64::MAX.to_string()
    } else {
        (rounded as u64).to_string()
    }
}

#[cfg(windows)]
fn reveal(dir: &Path) -> Result<(), String> {
    std::process::Command::new("explorer")
        .arg(dir)
        .spawn()
        .map_err(|err| format!("无法打开日志文件夹 ({err})"))?;
    Ok(())
}

#[cfg(not(windows))]
fn reveal(dir: &Path) -> Result<(), String> {
    let _ = dir;
    Err("无法打开日志文件夹".into())
}

#[cfg(windows)]
fn local_stamp() -> Stamp {
    #[repr(C)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(out: *mut SystemTime);
    }

    unsafe {
        let mut raw = std::mem::zeroed::<SystemTime>();
        GetLocalTime(&mut raw);
        Stamp {
            year: i32::from(raw.year),
            month: u32::from(raw.month),
            day: u32::from(raw.day),
            hour: u32::from(raw.hour),
            minute: u32::from(raw.minute),
            second: u32::from(raw.second),
        }
    }
}

#[cfg(not(windows))]
fn local_stamp() -> Stamp {
    use std::time::{SystemTime, UNIX_EPOCH};

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i32;
    let rem = (secs % 86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    Stamp {
        year,
        month,
        day,
        hour: rem / 3600,
        minute: (rem % 3600) / 60,
        second: rem % 60,
    }
}

#[cfg(not(windows))]
fn civil_from_days(z: i32) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i32 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn days_from_civil(mut year: i32, month: u32, day: u32) -> Option<i32> {
    if !valid_ymd(year, month, day) {
        return None;
    }
    year -= i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = (year - era * 400) as u32;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe as i32 - 719_468)
}

fn valid_ymd(year: i32, month: u32, day: u32) -> bool {
    let max = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap(year) => 29,
        2 => 28,
        _ => return false,
    };
    (1..=max).contains(&day)
}

fn leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn parse_log_day(name: &str) -> Option<i32> {
    let stem = name.strip_suffix(".csv")?;
    let (year, rest) = stem.split_once('-')?;
    let (month, day) = rest.split_once('-')?;
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    days_from_civil(year.parse().ok()?, month.parse().ok()?, day.parse().ok()?)
}

fn is_expired(today: i32, file_day: i32, keep_days: u16) -> bool {
    let span = i32::from(super::settings::normalize_keep_days(keep_days));
    file_day < today - (span - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch_and_leap_day() {
        assert_eq!(days_from_civil(1970, 1, 1), Some(0));
        assert_eq!(days_from_civil(2026, 2, 31), None);
        let leap = days_from_civil(2024, 3, 1).unwrap() - days_from_civil(2024, 2, 28).unwrap();
        let plain = days_from_civil(2025, 3, 1).unwrap() - days_from_civil(2025, 2, 28).unwrap();
        assert_eq!(leap, 2);
        assert_eq!(plain, 1);
    }

    #[test]
    fn retention_keeps_inclusive_calendar_days() {
        let today = days_from_civil(2026, 10, 24).unwrap();
        assert!(!is_expired(
            today,
            days_from_civil(2026, 10, 18).unwrap(),
            7
        ));
        assert!(is_expired(today, days_from_civil(2026, 10, 17).unwrap(), 7));
        assert!(!is_expired(
            today,
            days_from_civil(2026, 9, 25).unwrap(),
            30
        ));
        assert!(is_expired(today, days_from_civil(2026, 9, 24).unwrap(), 30));
        assert!(is_expired(today, today - 7, 14));
        assert!(!is_expired(today, today - 6, 14));

        let leap_today = days_from_civil(2024, 3, 1).unwrap();
        assert!(!is_expired(
            leap_today,
            days_from_civil(2024, 2, 24).unwrap(),
            7
        ));
        assert!(is_expired(
            leap_today,
            days_from_civil(2024, 2, 23).unwrap(),
            7
        ));
    }

    #[test]
    fn log_names_must_be_real_dates() {
        assert!(parse_log_day("2026-10-24.csv").is_some());
        assert_eq!(parse_log_day("2026-02-31.csv"), None);
        assert_eq!(parse_log_day("2026-10-24.txt"), None);
        assert_eq!(parse_log_day("notes.csv"), None);
        assert_eq!(parse_log_day("2026-1-02.csv"), None);
    }
}
