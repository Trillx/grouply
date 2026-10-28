//! The controller: turns window events and tab clicks into group changes and window moves.
//!
//! Inactive group members are never hidden. They are kept at the active window's position,
//! directly beneath it in the z-order, so the taskbar and Alt+Tab still list them, and nothing is
//! lost if Grouply exits or crashes.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Instant;

use windows::Win32::Foundation::{HINSTANCE, HWND};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::hooks::WinEvent;
use super::tabbar::{TabBar, TabView, ZOrder};
use super::win::{self, Win};
use crate::command::TabCommand;
use crate::drag::{DropTracker, SizeWatch};
use crate::handoff::SavedGroup;
use crate::layout::{self, Rect};
use crate::model::{GroupId, Groups, Removed};
use crate::settings::{MaximizedBar, Settings};

pub const TIMER_DRAG: usize = 1;
const DRAG_POLL_MS: u32 = 50;

struct Drag {
    window: Win,
    size: SizeWatch,
    tracker: DropTracker<Win>,
}

pub struct App {
    controller: HWND,
    instance: HINSTANCE,
    settings: Settings,
    light: bool,
    groups: Groups<Win>,
    bars: HashMap<GroupId, TabBar>,
    icons: HashMap<Win, windows::Win32::UI::WindowsAndMessaging::HICON>,
    drag: Option<Drag>,
    /// Groups minimized as a unit; their members' own minimize events are ours to ignore.
    minimized: HashSet<GroupId>,
    /// Members Grouply is restoring itself, whose restore events must not change the active tab.
    restoring: HashSet<Win>,
}

/// The window's visible width and height.
fn size(w: Win) -> Option<(i32, i32)> {
    win::frame(w).map(|f| (f.width(), f.height()))
}

fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

impl App {
    pub fn new(controller: HWND, instance: HINSTANCE, settings: Settings) -> Self {
        Self {
            controller,
            instance,
            settings,
            light: win::light_theme(),
            groups: Groups::new(),
            bars: HashMap::new(),
            icons: HashMap::new(),
            drag: None,
            minimized: HashSet::new(),
            restoring: HashSet::new(),
        }
    }

    pub fn on_event(&mut self, event: WinEvent) {
        match event {
            WinEvent::Foreground(w) => self.on_foreground(w),
            WinEvent::MoveSizeStart(w) => self.on_drag_start(w),
            WinEvent::MoveSizeEnd(w) => self.on_drag_end(w),
            WinEvent::MinimizeStart(w) => self.on_minimize(w),
            WinEvent::MinimizeEnd(w) => self.on_restore(w),
            WinEvent::Destroyed(w) | WinEvent::Hidden(w) => self.forget(w),
            WinEvent::Moved(w) => self.on_moved(w),
            WinEvent::Renamed(w) => {
                if let Some(id) = self.groups.group_of(w) {
                    self.refresh_bar(id);
                }
            }
            WinEvent::CloakChanged(w) => {
                if let Some(id) = self.groups.group_of(w) {
                    self.place_bar(id);
                }
            }
        }
    }

    fn on_foreground(&mut self, w: Win) {
        let Some(id) = self.groups.group_of(w) else {
            return;
        };
        if self.groups.activate(w).is_some() {
            self.restack(id);
            self.refresh_bar(id);
        } else {
            // The active window was raised above its bar; put the bar back on top of it.
            self.place_bar(id);
        }
    }

    fn on_moved(&mut self, w: Win) {
        let Some(id) = self.groups.group_of(w) else {
            return;
        };
        let is_active = self.groups.get(id).is_some_and(|g| g.active() == w);
        if is_active && !self.minimized.contains(&id) {
            self.sync(id);
        }
    }

    fn on_drag_start(&mut self, w: Win) {
        if !win::is_groupable(w) {
            return;
        }
        self.drag = Some(Drag {
            window: w,
            size: SizeWatch::new(size(w)),
            tracker: DropTracker::new(self.settings.hold_to_group_ms),
        });
        unsafe { SetTimer(Some(self.controller), TIMER_DRAG, DRAG_POLL_MS, None) };
    }

    pub fn on_drag_tick(&mut self) {
        let Some(dragged) = self.drag.as_ref().map(|d| d.window) else {
            self.stop_drag_timer();
            return;
        };
        let (x, y) = win::cursor_pos();
        let target = self.drop_target(dragged, x, y);
        if let Some(drag) = self.drag.as_mut() {
            drag.size.observe(size(dragged));
            drag.tracker.update(target, now_ms());
        }
    }

    /// Expands bars on maximized and top-snapped windows while the pointer is at the top edge,
    /// and tucks or hides them again when it leaves.
    pub fn on_hover_tick(&mut self) {
        if self.settings.maximized == MaximizedBar::Always {
            return;
        }
        let (x, y) = win::cursor_pos();
        for id in self.groups.ids() {
            let Some(active) = self.groups.get(id).map(|g| g.active()) else {
                continue;
            };
            let Some(frame) = win::frame(active) else {
                continue;
            };
            let work = win::work_area(active);
            let metrics = self.settings.metrics.scaled(win::dpi(active));
            if !layout::bar_overlays(frame, work, &metrics) {
                continue;
            }
            let Some(bar) = self.bars.get(&id) else {
                continue;
            };
            let revealed = layout::in_reveal_band(work, &metrics, x, y);
            // A bar stays as it is while one of its tabs is pressed.
            if revealed != bar.is_expanded() && !bar.is_pressed() {
                self.place_bar(id);
            }
        }
    }

    fn on_drag_end(&mut self, w: Win) {
        self.stop_drag_timer();
        let Some(mut drag) = self.drag.take() else {
            return;
        };
        if drag.window != w {
            return;
        }
        let resized = drag.size.resized(size(w));
        let (x, y) = win::cursor_pos();
        drag.tracker.update(self.drop_target(w, x, y), now_ms());
        match drag.tracker.armed(now_ms()) {
            Some(target) if !resized => self.merge(w, target),
            _ => {
                if let Some(id) = self.groups.group_of(w) {
                    self.sync(id);
                }
            }
        }
    }

    fn stop_drag_timer(&self) {
        let _ = unsafe { KillTimer(Some(self.controller), TIMER_DRAG) };
    }

    /// The window that `dragged` would group with if released at screen point (x, y).
    fn drop_target(&self, dragged: Win, x: i32, y: i32) -> Option<Win> {
        let own_group = self.groups.group_of(dragged);
        for (&id, bar) in &self.bars {
            if Some(id) != own_group && bar.visible_rect().is_some_and(|r| r.contains(x, y)) {
                return self.groups.get(id).map(|g| g.active());
            }
        }
        for w in win::top_level_windows() {
            if w == dragged || (own_group.is_some() && self.groups.group_of(w) == own_group) {
                continue;
            }
            // Bars were handled above; mostly transparent, they must not block what is below.
            if !win::is_opaque_surface(w) || self.bars.values().any(|b| b.hwnd() == w.hwnd()) {
                continue;
            }
            let Some(frame) = win::frame(w) else { continue };
            if !frame.contains(x, y) {
                continue;
            }
            // The first window under the cursor decides, groupable or not.
            if !win::is_groupable(w) || win::is_minimized(w) {
                return None;
            }
            let caption = self.settings.metrics.scaled(win::dpi(w)).height;
            return layout::in_drop_zone(frame, None, caption, x, y).then_some(w);
        }
        None
    }

    /// Groups `dragged` with `target`. The group takes over `target`'s position.
    fn merge(&mut self, dragged: Win, target: Win) {
        let anchor = self
            .groups
            .group_of(target)
            .and_then(|id| self.groups.get(id))
            .map_or(target, |g| g.active());
        let Some(frame) = win::frame(anchor) else {
            return;
        };
        let maximized = win::is_maximized(anchor);
        let Ok(merged) = self.groups.merge(dragged, target) else {
            return;
        };
        if let Some(gone) = merged.absorbed {
            self.bars.remove(&gone);
            self.minimized.remove(&gone);
        }
        let members = self
            .groups
            .get(merged.group)
            .map(|g| g.members().to_vec())
            .unwrap_or_default();
        for m in members {
            if m == dragged && maximized {
                win::maximize(m);
            } else {
                win::set_frame(m, frame);
            }
        }
        self.restack(merged.group);
        self.refresh_bar(merged.group);
    }

    fn on_minimize(&mut self, w: Win) {
        let Some(id) = self.groups.group_of(w) else {
            return;
        };
        if !self.minimized.insert(id) {
            return;
        }
        if let Some(group) = self.groups.get(id) {
            for m in group.members().iter().copied().filter(|&m| m != w) {
                win::minimize_quietly(m);
            }
        }
        if let Some(bar) = self.bars.get(&id) {
            bar.hide();
        }
    }

    fn on_restore(&mut self, w: Win) {
        let Some(id) = self.groups.group_of(w) else {
            return;
        };
        if !self.restoring.remove(&w) {
            self.groups.activate(w);
            if self.minimized.remove(&id)
                && let Some(group) = self.groups.get(id)
            {
                for m in group.inactive() {
                    if win::is_minimized(m) {
                        self.restoring.insert(m);
                        win::restore_quietly(m);
                    }
                }
            }
        }
        self.sync(id);
        self.restack(id);
        self.refresh_bar(id);
    }

    /// Drops a window that was closed or hidden from its group.
    fn forget(&mut self, w: Win) {
        self.icons.remove(&w);
        self.restoring.remove(&w);
        if self.drag.as_ref().is_some_and(|d| d.window == w) {
            self.drag = None;
            self.stop_drag_timer();
        }
        let removed = self.groups.remove(w);
        self.apply_removal(removed);
    }

    fn apply_removal(&mut self, removed: Removed<Win>) {
        match removed {
            Removed::NotGrouped => {}
            Removed::Dissolved { group, .. } => {
                self.bars.remove(&group);
                self.minimized.remove(&group);
            }
            Removed::Remains {
                group,
                active,
                active_changed,
            } => {
                if active_changed
                    && !self.minimized.contains(&group)
                    && let Some(bar) = self.bars.get(&group)
                {
                    // Raise the new active tab into the departed window's slot, under the bar.
                    win::place_below(active, Win::from(bar.hwnd()));
                }
                self.refresh_bar(group);
            }
        }
    }

    pub fn on_tab_command(&mut self, id: GroupId, cmd: TabCommand) {
        let member = |i: usize| {
            self.groups
                .get(id)
                .and_then(|g| g.members().get(i).copied())
        };
        match cmd {
            TabCommand::Activate(i) => self.switch_to(id, i),
            TabCommand::Close(i) => {
                if let Some(w) = member(i) {
                    win::close(w);
                }
            }
            TabCommand::Detach(i) => self.detach(id, i),
            TabCommand::Reorder { from, to } => {
                if self.groups.move_tab(id, from, to) {
                    self.refresh_bar(id);
                }
            }
            TabCommand::Ungroup => self.ungroup(id),
            TabCommand::CloseAll => {
                let members = self
                    .groups
                    .get(id)
                    .map(|g| g.members().to_vec())
                    .unwrap_or_default();
                for w in members {
                    win::close(w);
                }
            }
        }
    }

    fn switch_to(&mut self, id: GroupId, index: usize) {
        let Some(group) = self.groups.get(id) else {
            return;
        };
        let old = group.active();
        let Some(&new) = group.members().get(index) else {
            return;
        };
        if new != old {
            if win::is_maximized(old) {
                if !win::is_maximized(new) {
                    win::maximize(new);
                }
            } else if let Some(frame) = win::frame(old) {
                win::set_frame(new, frame);
            }
        }
        win::activate(new);
        if self.groups.activate(new).is_some() {
            win::place_below(old, new);
            self.refresh_bar(id);
        }
    }

    pub fn cycle(&mut self, forward: bool) {
        let Some(id) = win::foreground().and_then(|w| self.groups.group_of(w)) else {
            return;
        };
        let Some(next) = self.groups.neighbour(id, forward) else {
            return;
        };
        if let Some(index) = self.groups.get(id).and_then(|g| g.index_of(next)) {
            self.switch_to(id, index);
        }
    }

    /// Takes tab `index` out of its group and drops the window under the cursor.
    fn detach(&mut self, id: GroupId, index: usize) {
        let Some(w) = self
            .groups
            .get(id)
            .and_then(|g| g.members().get(index).copied())
        else {
            return;
        };
        let frame = win::frame(w);
        let removed = self.groups.remove(w);
        self.apply_removal(removed);
        if let Some(f) = frame {
            let m = self.settings.metrics.scaled(win::dpi(w));
            let (x, y) = win::cursor_pos();
            let (left, top) = (x - m.tab_max / 2, y - m.height / 2);
            win::set_frame(w, Rect::new(left, top, left + f.width(), top + f.height()));
        }
        win::activate(w);
    }

    /// Dissolves a group, cascading its windows so each is visible again.
    fn ungroup(&mut self, id: GroupId) {
        let active = self.groups.get(id).map(|g| g.active());
        let members = self.groups.dissolve(id);
        self.bars.remove(&id);
        self.minimized.remove(&id);
        let Some(origin) = active.and_then(win::frame) else {
            return;
        };
        let step = self
            .settings
            .metrics
            .scaled(active.map_or(96, win::dpi))
            .height;
        let mut offset = 0;
        for w in members.into_iter().filter(|&w| Some(w) != active) {
            offset += step;
            win::set_frame(w, origin.offset(offset, offset));
        }
    }

    pub fn ungroup_all(&mut self) {
        for id in self.groups.ids() {
            self.ungroup(id);
        }
    }

    pub fn refresh_theme(&mut self) {
        self.light = win::light_theme();
        for bar in self.bars.values() {
            bar.set_light(self.light);
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Applies changed settings to the running instance.
    pub fn apply_settings(&mut self, settings: Settings) {
        self.settings = settings;
        for bar in self.bars.values() {
            bar.set_options(self.settings.metrics, self.settings.middle_click_closes);
        }
        for id in self.groups.ids() {
            self.place_bar(id);
        }
    }

    /// Lines every inactive member up behind the active one.
    fn sync(&mut self, id: GroupId) {
        let Some(group) = self.groups.get(id) else {
            return;
        };
        let active = group.active();
        if win::is_minimized(active) {
            return;
        }
        let Some(frame) = win::frame(active) else {
            return;
        };
        for m in group.inactive() {
            if !win::is_minimized(m) && win::frame(m) != Some(frame) {
                win::set_frame(m, frame);
            }
        }
        self.place_bar(id);
    }

    fn restack(&mut self, id: GroupId) {
        let Some(group) = self.groups.get(id) else {
            return;
        };
        let active = group.active();
        for m in group.inactive() {
            win::place_below(m, active);
        }
        self.place_bar(id);
    }

    /// Rebuilds the bar's tabs from the group (creating the bar if needed) and positions it.
    fn refresh_bar(&mut self, id: GroupId) {
        let Some(group) = self.groups.get(id) else {
            return;
        };
        let members = group.members().to_vec();
        let active = group.active_index();
        if !self.bars.contains_key(&id) {
            let created = TabBar::new(
                self.instance,
                self.controller,
                id,
                self.settings.metrics,
                self.light,
                self.settings.middle_click_closes,
            );
            match created {
                Ok(bar) => {
                    self.bars.insert(id, bar);
                }
                Err(_) => return,
            }
        }
        let tabs = members
            .iter()
            .map(|&w| TabView {
                title: win::title(w),
                icon: *self.icons.entry(w).or_insert_with(|| win::small_icon(w)),
            })
            .collect();
        if let Some(bar) = self.bars.get(&id) {
            bar.set_tabs(tabs, active);
        }
        self.place_bar(id);
    }

    fn place_bar(&self, id: GroupId) {
        let (Some(group), Some(bar)) = (self.groups.get(id), self.bars.get(&id)) else {
            return;
        };
        let active = group.active();
        if self.minimized.contains(&id)
            || win::is_minimized(active)
            || !win::is_visible(active)
            || win::is_cloaked(active)
        {
            bar.hide();
            return;
        }
        let Some(frame) = win::frame(active) else {
            return;
        };
        let work = win::work_area(active);
        let dpi = win::dpi(active);
        let metrics = self.settings.metrics.scaled(dpi);
        let mut tucked = false;
        // Collapsing mid-press would pull the tab out from under a reorder or detach drag.
        if self.settings.maximized != MaximizedBar::Always
            && layout::bar_overlays(frame, work, &metrics)
            && !bar.is_pressed()
        {
            let (x, y) = win::cursor_pos();
            if !layout::in_reveal_band(work, &metrics, x, y) {
                if self.settings.maximized == MaximizedBar::Hidden {
                    bar.hide();
                    return;
                }
                tucked = true;
            }
        }
        let z = match win::above(active) {
            Some(w) if w.hwnd() == bar.hwnd() => ZOrder::Keep,
            // Inserting after a topmost window would make the bar topmost too.
            Some(w) if !win::is_topmost(w) => ZOrder::After(w),
            _ => ZOrder::TopNormal,
        };
        bar.place(frame, work, dpi, z, tucked);
    }

    /// The current groups, for the next instance to adopt after an update.
    pub fn saved_groups(&self) -> Vec<SavedGroup> {
        self.groups
            .ids()
            .into_iter()
            .filter_map(|id| self.groups.get(id))
            .map(|g| SavedGroup {
                members: g.members().iter().map(|w| w.0).collect(),
                active: g.active_index(),
            })
            .collect()
    }

    /// Regroups windows handed over by the previous instance. Windows that have since closed or
    /// can no longer be managed are left out.
    pub fn adopt(&mut self, saved: &[SavedGroup]) {
        for group in saved {
            let members: Vec<Win> = group
                .members
                .iter()
                .map(|&h| Win(h))
                .filter(|&w| win::is_groupable(w) && self.groups.group_of(w).is_none())
                .collect();
            let Some((&first, rest)) = members.split_first() else {
                continue;
            };
            let mut id = None;
            for &w in rest {
                if let Ok(merged) = self.groups.merge(w, first) {
                    id = Some(merged.group);
                }
            }
            let Some(id) = id else { continue };
            let active = group
                .members
                .get(group.active)
                .map(|&h| Win(h))
                .filter(|w| members.contains(w))
                .unwrap_or(first);
            self.groups.activate(active);
            if win::is_minimized(active) {
                self.minimized.insert(id);
            }
            self.sync(id);
            self.restack(id);
            self.refresh_bar(id);
        }
    }

    /// Ungroups everything on exit. Members were never hidden, so nothing needs restoring.
    pub fn shutdown(&mut self) {
        self.stop_drag_timer();
        for id in self.groups.ids() {
            self.groups.dissolve(id);
        }
        self.bars.clear();
    }
}
