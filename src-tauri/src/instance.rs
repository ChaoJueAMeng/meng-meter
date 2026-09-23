use std::sync::atomic::{AtomicIsize, Ordering};

pub fn acquire() {
    #[cfg(windows)]
    windows::acquire();
}

pub fn listen(on_focus: impl Fn() + Send + 'static) {
    #[cfg(windows)]
    windows::listen(on_focus);
    #[cfg(not(windows))]
    let _ = on_focus;
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::thread;

    const MUTEX_NAME: &str = r"Local\com.meng.meter.instance";
    const EVENT_NAME: &str = r"Local\com.meng.meter.focus";
    const ERROR_ALREADY_EXISTS: u32 = 183;
    const EVENT_MODIFY_STATE: u32 = 0x0002;
    const INFINITE: u32 = 0xFFFF_FFFF;
    const WAIT_OBJECT_0: u32 = 0;

    static MUTEX: AtomicIsize = AtomicIsize::new(0);

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn acquire() {
        unsafe {
            SetLastError(0);
            let name = wide(MUTEX_NAME);
            let handle = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
            if handle == 0 {
                return;
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(handle);
                signal_focus();
                std::process::exit(0);
            }
            MUTEX.store(handle, Ordering::SeqCst);
        }
    }

    fn signal_focus() {
        unsafe {
            let name = wide(EVENT_NAME);
            let event = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
            if event == 0 {
                return;
            }
            SetEvent(event);
            CloseHandle(event);
        }
    }

    pub fn listen(on_focus: impl Fn() + Send + 'static) {
        thread::spawn(move || {
            let event = unsafe {
                let name = wide(EVENT_NAME);
                CreateEventW(std::ptr::null(), 0, 0, name.as_ptr())
            };
            if event == 0 {
                return;
            }
            loop {
                let wait = unsafe { WaitForSingleObject(event, INFINITE) };
                if wait != WAIT_OBJECT_0 {
                    break;
                }
                on_focus();
            }
        });
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(attr: *const u8, initial_owner: i32, name: *const u16) -> isize;
        fn CreateEventW(
            attr: *const u8,
            manual_reset: i32,
            initial_state: i32,
            name: *const u16,
        ) -> isize;
        fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> isize;
        fn SetEvent(event: isize) -> i32;
        fn WaitForSingleObject(handle: isize, millis: u32) -> u32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetLastError() -> u32;
        fn SetLastError(code: u32);
    }
}
