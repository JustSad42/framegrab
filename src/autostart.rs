//! "Start with Windows" via the per-user Run key. Dev builds and test copies never register.

use windows::Win32::System::Registry::*;
use windows_core::PCWSTR;

const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const NAME: &str = "Framegrab";

pub fn is_dev_build() -> bool {
    std::env::current_exe().map(|p| p.to_string_lossy().to_lowercase().contains(r"\target\")).unwrap_or(true)
}

pub fn apply(enabled: bool) {
    if is_dev_build() || crate::config::test_mode() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let key = crate::win::wide(KEY);
    let name = crate::win::wide(NAME);
    unsafe {
        let mut hk = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), None, KEY_SET_VALUE | KEY_QUERY_VALUE, &mut hk).is_err() {
            return;
        }
        if enabled {
            let val = crate::win::wide(&format!("\"{}\" --background", exe.display()));
            let bytes = std::slice::from_raw_parts(val.as_ptr() as *const u8, val.len() * 2);
            let _ = RegSetValueExW(hk, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes));
        } else {
            let _ = RegDeleteValueW(hk, PCWSTR(name.as_ptr()));
        }
        let _ = RegCloseKey(hk);
    }
}
