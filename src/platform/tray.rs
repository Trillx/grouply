//! Notification-area icon: the only always-visible UI. It shows the version and update status,
//! and is how the user updates and exits.

use std::mem::size_of;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

use super::win;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayChoice {
    Update,
    CheckForUpdates,
    UngroupAll,
    Exit,
}

/// The menu's dynamic lines.
pub struct MenuInfo {
    pub version: String,
    pub status: String,
    pub can_update: bool,
    pub can_check: bool,
}

pub struct Tray {
    data: NOTIFYICONDATAW,
}

/// Copies `text` into a fixed-size, NUL-terminated UTF-16 field, truncating if needed.
fn fill(field: &mut [u16], text: &str) {
    field.fill(0);
    let room = field.len() - 1;
    for (dst, src) in field[..room].iter_mut().zip(text.encode_utf16()) {
        *dst = src;
    }
}

impl Tray {
    /// Adds the icon. Failure is tolerated: Explorer may not be running yet, and the icon is
    /// re-added when the taskbar announces itself.
    pub fn add(owner: HWND, callback_message: u32, tooltip: &str) -> Self {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: owner,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: callback_message,
            hIcon: unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default(),
            ..Default::default()
        };
        fill(&mut data.szTip, tooltip);
        let tray = Self { data };
        tray.show();
        tray
    }

    /// (Re-)adds the icon, e.g. after Explorer restarts.
    pub fn show(&self) {
        let _ = unsafe { Shell_NotifyIconW(NIM_ADD, &self.data) };
    }

    pub fn set_tooltip(&mut self, tooltip: &str) {
        fill(&mut self.data.szTip, tooltip);
        let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &self.data) };
    }

    /// Shows a notification from the icon. Clicking it sends `NIN_BALLOONUSERCLICK`.
    pub fn notify(&self, title: &str, text: &str) {
        // A copy, so re-adding the icon later doesn't repeat the notification.
        let mut data = self.data;
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        fill(&mut data.szInfoTitle, title);
        fill(&mut data.szInfo, text);
        let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &self.data) };
    }
}

pub fn show_menu(owner: HWND, info: &MenuInfo) -> Option<TrayChoice> {
    const UPDATE: usize = 1;
    const CHECK: usize = 2;
    const UNGROUP_ALL: usize = 3;
    const EXIT: usize = 4;
    let enabled = |on: bool| if on { MF_STRING } else { MF_STRING | MF_GRAYED };
    let menu = unsafe { CreatePopupMenu() }.ok()?;
    let choice = unsafe {
        let _ = AppendMenuW(menu, MF_GRAYED, 0, &HSTRING::from(&info.version));
        let _ = AppendMenuW(menu, MF_GRAYED, 0, &HSTRING::from(&info.status));
        let _ = AppendMenuW(
            menu,
            enabled(info.can_update),
            UPDATE,
            w!("Update and restart"),
        );
        let _ = AppendMenuW(
            menu,
            enabled(info.can_check),
            CHECK,
            w!("Check for updates"),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, UNGROUP_ALL, w!("Ungroup all windows"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, EXIT, w!("Exit Grouply"));
        let (x, y) = win::cursor_pos();
        // Without this the menu stays open when the user clicks elsewhere.
        let _ = SetForegroundWindow(owner);
        let choice = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            x,
            y,
            None,
            owner,
            None,
        );
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        choice.0 as usize
    };
    match choice {
        UPDATE => Some(TrayChoice::Update),
        CHECK => Some(TrayChoice::CheckForUpdates),
        UNGROUP_ALL => Some(TrayChoice::UngroupAll),
        EXIT => Some(TrayChoice::Exit),
        _ => None,
    }
}
