use serde::{Deserialize, Serialize};
use std::fs;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default = "enabled")]
    pub floating: bool,
    #[serde(default = "enabled")]
    pub taskbar_temp: bool,
}

fn enabled() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            configured: false,
            autostart: false,
            floating: true,
            taskbar_temp: true,
        }
    }
}

fn path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_config_dir().ok()?;
    Some(dir.join("settings.json"))
}

pub fn load(app: &AppHandle) -> Settings {
    let Some(path) = path(app) else {
        return Settings::default();
    };
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = path(app).ok_or("无法定位配置目录")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(path, bytes).map_err(|e| e.to_string())
}

pub fn apply(app: &AppHandle, settings: &Settings) {
    let _ = set_autostart(settings.autostart);
    apply_floating(app, settings.floating);
    crate::taskbar::set_enabled(app, settings.taskbar_temp);
}

fn apply_floating(app: &AppHandle, enabled: bool) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    if enabled {
        let _ = win.show();
        let _ = win.set_always_on_top(true);
    } else {
        let _ = win.hide();
    }
}

pub fn set_autostart(enable: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        autostart::set_enabled(enable)
    }
    #[cfg(not(windows))]
    {
        let _ = enable;
        Ok(())
    }
}

#[cfg(windows)]
mod autostart {
    const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as isize;
    const KEY_SET_VALUE: u32 = 0x0002;
    const REG_OPTION_NON_VOLATILE: u32 = 0;
    const REG_SZ: u32 = 1;
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const VALUE_NAME: &str = "MengMeter";
    const SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn set_enabled(enable: bool) -> Result<(), String> {
        unsafe {
            let mut key = 0isize;
            let subkey = wide(SUBKEY);
            let status = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            );
            if status != 0 {
                return Err(format!("无法写入开机启动项 ({status})"));
            }
            let name = wide(VALUE_NAME);
            let result = if enable {
                let exe = std::env::current_exe().map_err(|e| {
                    RegCloseKey(key);
                    e.to_string()
                })?;
                let cmd = format!("\"{}\"", exe.display());
                let data = wide(&cmd);
                let bytes = (data.len() * 2) as u32;
                RegSetValueExW(
                    key,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    data.as_ptr() as *const u8,
                    bytes,
                )
            } else {
                RegDeleteValueW(key, name.as_ptr())
            };
            RegCloseKey(key);
            if result != 0 && !( !enable && result == ERROR_FILE_NOT_FOUND) {
                return Err(format!("无法更新开机启动项 ({result})"));
            }
            Ok(())
        }
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn RegCreateKeyExW(
            key: isize,
            sub_key: *const u16,
            reserved: u32,
            class: *const u16,
            options: u32,
            sam: u32,
            security: *const u8,
            result: *mut isize,
            disposition: *mut u32,
        ) -> i32;
        fn RegSetValueExW(
            key: isize,
            value_name: *const u16,
            reserved: u32,
            ty: u32,
            data: *const u8,
            data_len: u32,
        ) -> i32;
        fn RegDeleteValueW(key: isize, value_name: *const u16) -> i32;
        fn RegCloseKey(key: isize) -> i32;
    }
}
