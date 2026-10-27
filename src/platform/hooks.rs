//! System-wide window events via out-of-context WinEvent hooks.
//!
//! Out-of-context hooks need no DLL in the target process, so one native ARM64 Grouply sees
//! events from every application regardless of its architecture. The callback runs on our UI
//! thread; it only queues the event and wakes the controller, which handles it outside the hook.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::win::Win;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WinEvent {
    Foreground(Win),
    MoveSizeStart(Win),
    MoveSizeEnd(Win),
    MinimizeStart(Win),
    MinimizeEnd(Win),
    Destroyed(Win),
    Hidden(Win),
    Moved(Win),
    Renamed(Win),
    /// Hidden or shown by the shell, e.g. when switching virtual desktops.
    CloakChanged(Win),
}

/// Posted to the controller window when events are waiting in the queue.
pub const WM_APP_EVENTS: u32 = WM_APP + 1;

thread_local! {
    static QUEUE: RefCell<VecDeque<WinEvent>> = const { RefCell::new(VecDeque::new()) };
    static SINK: Cell<isize> = const { Cell::new(0) };
    static POSTED: Cell<bool> = const { Cell::new(false) };
}

pub struct Hooks(Vec<HWINEVENTHOOK>);

impl Hooks {
    pub fn install(sink: HWND) -> windows::core::Result<Self> {
        SINK.set(sink.0 as isize);
        let ranges = [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MINIMIZEEND),
            (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
            (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_NAMECHANGE),
            (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
        ];
        let mut hooks = Self(Vec::with_capacity(ranges.len()));
        for (min, max) in ranges {
            let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
            let hook = unsafe { SetWinEventHook(min, max, None, Some(callback), 0, 0, flags) };
            if hook.is_invalid() {
                return Err(windows::core::Error::from_thread());
            }
            hooks.0.push(hook);
        }
        Ok(hooks)
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        for &hook in &self.0 {
            let _ = unsafe { UnhookWinEvent(hook) };
        }
    }
}

/// Takes every queued event. Call when `WM_APP_EVENTS` arrives.
pub fn take() -> Vec<WinEvent> {
    POSTED.set(false);
    QUEUE.with_borrow_mut(|q| q.drain(..).collect())
}

/// Re-announces queued events that could not be handled yet.
pub fn repost() {
    POSTED.set(false);
    if QUEUE.with_borrow(|q| !q.is_empty()) {
        announce();
    }
}

fn announce() {
    if POSTED.replace(true) {
        return;
    }
    let sink = HWND(SINK.get() as *mut _);
    if unsafe { PostMessageW(Some(sink), WM_APP_EVENTS, WPARAM(0), LPARAM(0)) }.is_err() {
        POSTED.set(false);
    }
}

unsafe extern "system" fn callback(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    // Only whole windows matter, not carets, cursors, scroll bars or other accessible objects.
    if hwnd.is_invalid() || id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 {
        return;
    }
    let w = Win::from(hwnd);
    let event = match event {
        EVENT_SYSTEM_FOREGROUND => WinEvent::Foreground(w),
        EVENT_SYSTEM_MOVESIZESTART => WinEvent::MoveSizeStart(w),
        EVENT_SYSTEM_MOVESIZEEND => WinEvent::MoveSizeEnd(w),
        EVENT_SYSTEM_MINIMIZESTART => WinEvent::MinimizeStart(w),
        EVENT_SYSTEM_MINIMIZEEND => WinEvent::MinimizeEnd(w),
        EVENT_OBJECT_DESTROY => WinEvent::Destroyed(w),
        EVENT_OBJECT_HIDE => WinEvent::Hidden(w),
        EVENT_OBJECT_LOCATIONCHANGE => WinEvent::Moved(w),
        EVENT_OBJECT_NAMECHANGE => WinEvent::Renamed(w),
        EVENT_OBJECT_CLOAKED | EVENT_OBJECT_UNCLOAKED => WinEvent::CloakChanged(w),
        _ => return,
    };
    let queued = QUEUE.with_borrow_mut(|q| {
        // Geometry is read when the event is handled, so one pending move per window suffices.
        if matches!(event, WinEvent::Moved(_)) && q.contains(&event) {
            return false;
        }
        q.push_back(event);
        true
    });
    if queued {
        announce();
    }
}
