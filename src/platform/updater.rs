//! Runs update jobs off the UI thread and swaps in a downloaded build.
//!
//! A running executable can't be overwritten but can be renamed. So the update is staged as
//! `grouply.exe.new` beside it, the running file is renamed to `.old`, the new one takes its name,
//! and the new instance is started with the current groups. It waits for this one to exit, then
//! deletes `.old`.

use std::ffi::c_void;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use std::{fs, io, ptr, thread};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{HSTRING, PCWSTR, w};

use crate::handoff::{Launch, SavedGroup};
use crate::update::{self, Asset, Job, Manifest};

pub const WM_APP_UPDATE: u32 = WM_APP + 4;
const MAX_MANIFEST: usize = 64 * 1024;
const MAX_EXECUTABLE: usize = 64 * 1024 * 1024;

pub enum Outcome {
    Checked(Result<Manifest, String>),
    Downloaded(Result<PathBuf, String>),
}

/// Runs `job` on a worker thread and posts its outcome to `controller` as `WM_APP_UPDATE`.
pub fn start(controller: HWND, job: Job) {
    let target = controller.0 as isize;
    thread::spawn(move || {
        let outcome = match job {
            Job::Check { url } => Outcome::Checked(fetch_manifest(&url)),
            Job::Download(asset) => Outcome::Downloaded(fetch_build(&asset)),
        };
        let outcome = Box::into_raw(Box::new(outcome));
        let posted = unsafe {
            PostMessageW(
                Some(HWND(target as _)),
                WM_APP_UPDATE,
                WPARAM(0),
                LPARAM(outcome as isize),
            )
        };
        if posted.is_err() {
            drop(unsafe { Box::from_raw(outcome) });
        }
    });
}

/// Reclaims the outcome carried by a `WM_APP_UPDATE` message.
///
/// # Safety
/// `lparam` must come from a `WM_APP_UPDATE` posted by `start`, and be taken only once.
pub unsafe fn take(lparam: LPARAM) -> Outcome {
    *unsafe { Box::from_raw(lparam.0 as *mut Outcome) }
}

fn fetch_manifest(url: &str) -> Result<Manifest, String> {
    let body = http_get(url, MAX_MANIFEST)?;
    let text = String::from_utf8_lossy(&body);
    Manifest::parse(&text).ok_or_else(|| "the update manifest is malformed".into())
}

/// Downloads and verifies a build, then stages it beside the running executable.
fn fetch_build(asset: &Asset) -> Result<PathBuf, String> {
    let bytes = http_get(&asset.url, MAX_EXECUTABLE)?;
    if update::hex(&sha256(&bytes)?) != asset.sha256 {
        return Err("the download is corrupt (checksum mismatch)".into());
    }
    if update::pe_machine(&bytes) != Some(update::PE_MACHINE) {
        return Err("the download is not a Grouply build for this processor".into());
    }
    let staged = sibling(&current_exe()?, "new");
    fs::write(&staged, &bytes).map_err(|e| file_error("save the update", &e))?;
    Ok(staged)
}

/// Puts the staged build in place of the running executable and starts it with `groups`.
/// On success the caller must exit promptly: the new instance waits for it.
pub fn install(staged: &Path, groups: &[SavedGroup]) -> Result<(), String> {
    let exe = current_exe()?;
    let old = sibling(&exe, "old");
    fs::rename(&exe, &old).map_err(|e| file_error("replace the running program", &e))?;
    if let Err(e) = fs::rename(staged, &exe) {
        let _ = fs::rename(&old, &exe);
        return Err(file_error("move the update into place", &e));
    }
    if let Err(e) = Command::new(&exe).args(Launch::args_for(groups)).spawn() {
        let _ = fs::rename(&exe, staged);
        let _ = fs::rename(&old, &exe);
        return Err(format!("couldn't start the new version: {e}"));
    }
    Ok(())
}

/// Deletes what an earlier update left behind. The previous executable may stay locked for a
/// moment after its process exits, so this retries in the background.
pub fn remove_leftovers() {
    let Ok(exe) = current_exe() else {
        return;
    };
    let _ = fs::remove_file(sibling(&exe, "new"));
    let old = sibling(&exe, "old");
    thread::spawn(move || {
        for _ in 0..20 {
            match fs::remove_file(&old) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => {
                    thread::sleep(Duration::from_millis(500))
                }
                _ => return,
            }
        }
    });
}

fn current_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("couldn't locate Grouply's executable: {e}"))
}

/// `grouply.exe` → `grouply.exe.<suffix>`.
fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}

fn file_error(action: &str, error: &io::Error) -> String {
    if error.kind() == io::ErrorKind::PermissionDenied {
        format!(
            "couldn't {action}: access denied. Move Grouply to a folder you can write to, such as one under your user profile."
        )
    } else {
        format!("couldn't {action}: {error}")
    }
}

fn sha256(bytes: &[u8]) -> Result<[u8; 32], String> {
    let mut digest = [0u8; 32];
    unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, bytes, &mut digest) }
        .ok()
        .map_err(|e| format!("couldn't verify the download: {e}"))?;
    Ok(digest)
}

/// A WinHTTP handle, closed on drop.
struct Internet(*mut c_void);

impl Internet {
    fn new(handle: *mut c_void) -> Result<Self, String> {
        if handle.is_null() {
            Err(network_error(windows::core::Error::from_thread()))
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for Internet {
    fn drop(&mut self) {
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn network_error(error: windows::core::Error) -> String {
    // WinHTTP's ERROR_WINHTTP_* codes have no system message text.
    let code = error.code().0 as u32 & 0xFFFF;
    let reason = match code {
        12002 => "the connection timed out",
        12007 => "the update server could not be found (offline?)",
        12029 | 12030 => "couldn't connect to the update server",
        12175 => "the secure connection failed",
        _ => return format!("network error {code}"),
    };
    reason.into()
}

/// GETs `url` over HTTPS, following redirects, and returns the body if the final status is 200.
fn http_get(url: &str, limit: usize) -> Result<Vec<u8>, String> {
    let (host, path) = update::split_https_url(url).ok_or("unsupported update URL")?;
    unsafe {
        let session = Internet::new(WinHttpOpen(
            w!("Grouply-Updater"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        let _ = WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 60_000);
        let connection = Internet::new(WinHttpConnect(
            session.0,
            &HSTRING::from(host),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ))?;
        let request = Internet::new(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(network_error)?;
        WinHttpReceiveResponse(request.0, ptr::null_mut()).map_err(network_error)?;

        let mut status = 0u32;
        let mut length = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &mut length,
            ptr::null_mut(),
        )
        .map_err(network_error)?;
        if status != 200 {
            return Err(format!("the update server answered HTTP {status}"));
        }

        let mut body = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &mut read,
            )
            .map_err(network_error)?;
            if read == 0 {
                return Ok(body);
            }
            if body.len() + read as usize > limit {
                return Err("the download is larger than expected".into());
            }
            body.extend_from_slice(&chunk[..read as usize]);
        }
    }
}
