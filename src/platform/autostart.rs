//! The "Start with Windows" entry: one value under `HKCU\...\CurrentVersion\Run`.
//!
//! Writing to HKCU needs no elevation and is undone by clearing the same value. The entry is read
//! back from the registry rather than mirrored in settings, so changes made with `regedit` or
//! another tool show up in the menu too.

use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW,
};
use windows::core::{PCWSTR, w};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE_NAME: PCWSTR = w!("Grouply");
/// Longest command line the menu will accept; the registry itself has no such limit.
const MAX_VALUE: usize = 1024;

/// Whether a start-with-Windows entry exists.
pub fn is_enabled() -> bool {
    command().is_some()
}

/// The command line stored in the Run key, if any.
pub fn command() -> Option<String> {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            Some(0),
            KEY_QUERY_VALUE,
            &mut key,
        )
        .is_err()
        {
            return None;
        }
        let mut buf = [0u16; MAX_VALUE];
        let mut bytes = std::mem::size_of_val(&buf) as u32;
        let mut kind = REG_VALUE_TYPE(0);
        let status = RegQueryValueExW(
            key,
            VALUE_NAME,
            None,
            Some(&mut kind),
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        );
        let _ = RegCloseKey(key);
        if status.is_err() || kind != REG_SZ || bytes < 2 {
            return None;
        }
        let units = (bytes as usize / 2).min(buf.len());
        let text = String::from_utf16_lossy(&buf[..units]);
        let text = text.trim_end_matches('\0');
        (!text.is_empty()).then(|| text.to_owned())
    }
}

/// Adds or removes the entry for this executable. The path is quoted, as Windows expects when it
/// contains spaces.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    unsafe {
        let mut key = HKEY::default();
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_QUERY_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()
        .map_err(|e| format!("couldn't open the sign-in registry key: {e}"))?;

        let status = if enabled {
            match std::env::current_exe() {
                Ok(exe) => {
                    let command = format!("\"{}\"", exe.display());
                    let wide: Vec<u16> = command.encode_utf16().chain([0]).collect();
                    // SAFETY: the slice describes `wide` for the duration of the call.
                    let bytes =
                        std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2);
                    RegSetValueExW(key, VALUE_NAME, None, REG_SZ, Some(bytes))
                }
                Err(error) => {
                    let _ = RegCloseKey(key);
                    return Err(format!("couldn't locate Grouply's executable: {error}"));
                }
            }
        } else {
            let status = RegDeleteValueW(key, VALUE_NAME);
            // Removing an entry that was never there is success.
            if status == ERROR_FILE_NOT_FOUND {
                windows::Win32::Foundation::WIN32_ERROR(0)
            } else {
                status
            }
        };
        let _ = RegCloseKey(key);
        status
            .ok()
            .map_err(|e| format!("couldn't change the sign-in entry: {e}"))
    }
}
