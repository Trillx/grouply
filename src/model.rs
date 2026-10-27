//! Window groups, independent of any windowing system.
//!
//! A group always has at least two members; removing the second-to-last member dissolves it.
//! `W` is a window identifier (an `HWND` wrapper on Windows, plain integers in tests).

use std::collections::HashMap;
use std::hash::Hash;

pub type GroupId = u32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group<W> {
    members: Vec<W>,
    active: usize,
}

impl<W: Copy + Eq> Group<W> {
    pub fn members(&self) -> &[W] {
        &self.members
    }

    pub fn active(&self) -> W {
        self.members[self.active]
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn index_of(&self, w: W) -> Option<usize> {
        self.members.iter().position(|&m| m == w)
    }

    /// Members other than the active one.
    pub fn inactive(&self) -> impl Iterator<Item = W> + '_ {
        let active = self.active();
        self.members.iter().copied().filter(move |&m| m != active)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeError {
    SameWindow,
    AlreadyTogether,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Merged {
    pub group: GroupId,
    /// The dragged window's former group, now folded into `group` and gone.
    pub absorbed: Option<GroupId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removed<W> {
    NotGrouped,
    /// The group lives on. `active_changed` is true when the removed window was the active one.
    Remains {
        group: GroupId,
        active: W,
        active_changed: bool,
    },
    /// Only `survivor` was left, so the group no longer exists.
    Dissolved {
        group: GroupId,
        survivor: W,
    },
}

#[derive(Debug)]
pub struct Groups<W> {
    groups: HashMap<GroupId, Group<W>>,
    membership: HashMap<W, GroupId>,
    next_id: GroupId,
}

impl<W: Copy + Eq + Hash> Default for Groups<W> {
    fn default() -> Self {
        Self {
            groups: HashMap::new(),
            membership: HashMap::new(),
            next_id: 1,
        }
    }
}

impl<W: Copy + Eq + Hash> Groups<W> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn group_of(&self, w: W) -> Option<GroupId> {
        self.membership.get(&w).copied()
    }

    pub fn get(&self, id: GroupId) -> Option<&Group<W>> {
        self.groups.get(&id)
    }

    pub fn ids(&self) -> Vec<GroupId> {
        let mut ids: Vec<_> = self.groups.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Drop `dragged` onto `target`.
    ///
    /// Everything grouped with `target` (or `target` alone) comes first, followed by everything
    /// grouped with `dragged` (or `dragged` alone). The dragged window becomes active. The
    /// surviving group keeps `target`'s group id when it had one.
    pub fn merge(&mut self, dragged: W, target: W) -> Result<Merged, MergeError> {
        if dragged == target {
            return Err(MergeError::SameWindow);
        }
        let target_group = self.group_of(target);
        let dragged_group = self.group_of(dragged);
        if target_group.is_some() && target_group == dragged_group {
            return Err(MergeError::AlreadyTogether);
        }

        let mut members = match target_group {
            Some(id) => self
                .groups
                .remove(&id)
                .map(|g| g.members)
                .unwrap_or_default(),
            None => vec![target],
        };
        let incoming = match dragged_group {
            Some(id) => self
                .groups
                .remove(&id)
                .map(|g| g.members)
                .unwrap_or_default(),
            None => vec![dragged],
        };
        members.extend(incoming);

        let id = target_group
            .or(dragged_group)
            .unwrap_or_else(|| self.allocate_id());
        let active = members.iter().position(|&m| m == dragged).unwrap_or(0);
        for &m in &members {
            self.membership.insert(m, id);
        }
        self.groups.insert(id, Group { members, active });

        let absorbed = dragged_group.filter(|&d| d != id);
        Ok(Merged {
            group: id,
            absorbed,
        })
    }

    pub fn remove(&mut self, w: W) -> Removed<W> {
        let Some(id) = self.membership.remove(&w) else {
            return Removed::NotGrouped;
        };
        let group = self
            .groups
            .get_mut(&id)
            .expect("membership points at a live group");
        let index = group.index_of(w).expect("member listed in its group");
        group.members.remove(index);

        if group.members.len() == 1 {
            let survivor = group.members[0];
            self.groups.remove(&id);
            self.membership.remove(&survivor);
            return Removed::Dissolved {
                group: id,
                survivor,
            };
        }

        let active_changed = index == group.active;
        if index < group.active || (active_changed && group.active == group.members.len()) {
            group.active -= 1;
        }
        Removed::Remains {
            group: id,
            active: group.active(),
            active_changed,
        }
    }

    /// Makes `w` its group's active window. Returns the group when the active window changed.
    pub fn activate(&mut self, w: W) -> Option<GroupId> {
        let id = self.group_of(w)?;
        let group = self.groups.get_mut(&id)?;
        let index = group.index_of(w)?;
        if index == group.active {
            return None;
        }
        group.active = index;
        Some(id)
    }

    /// The member next to the active one, wrapping around. Does not change the active window.
    pub fn neighbour(&self, id: GroupId, forward: bool) -> Option<W> {
        let group = self.groups.get(&id)?;
        let n = group.members.len();
        let index = if forward {
            (group.active + 1) % n
        } else {
            (group.active + n - 1) % n
        };
        Some(group.members[index])
    }

    /// Moves the tab at `from` to `to`, keeping the same window active.
    pub fn move_tab(&mut self, id: GroupId, from: usize, to: usize) -> bool {
        let Some(group) = self.groups.get_mut(&id) else {
            return false;
        };
        let n = group.members.len();
        if from >= n || to >= n || from == to {
            return false;
        }
        let active = group.active();
        let w = group.members.remove(from);
        group.members.insert(to, w);
        group.active = group.index_of(active).expect("active window still present");
        true
    }

    /// Ungroups every member of `id`, returning them in tab order.
    pub fn dissolve(&mut self, id: GroupId) -> Vec<W> {
        let Some(group) = self.groups.remove(&id) else {
            return Vec::new();
        };
        for m in &group.members {
            self.membership.remove(m);
        }
        group.members
    }

    fn allocate_id(&mut self) -> GroupId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members(groups: &Groups<u32>, id: GroupId) -> Vec<u32> {
        groups.get(id).unwrap().members().to_vec()
    }

    #[test]
    fn dropping_one_window_on_another_creates_a_group() {
        let mut g = Groups::new();
        let m = g.merge(2, 1).unwrap();
        assert_eq!(m.absorbed, None);
        assert_eq!(members(&g, m.group), vec![1, 2]);
        assert_eq!(g.get(m.group).unwrap().active(), 2);
        assert_eq!(g.group_of(1), Some(m.group));
        assert_eq!(g.group_of(2), Some(m.group));
    }

    #[test]
    fn dropping_onto_a_grouped_window_appends_and_activates() {
        let mut g = Groups::new();
        let id = g.merge(2, 1).unwrap().group;
        g.activate(1);
        let m = g.merge(3, 1).unwrap();
        assert_eq!(m.group, id);
        assert_eq!(members(&g, id), vec![1, 2, 3]);
        assert_eq!(g.get(id).unwrap().active(), 3);
    }

    #[test]
    fn dropping_a_group_onto_a_window_keeps_the_groups_id() {
        let mut g = Groups::new();
        let id = g.merge(2, 1).unwrap().group;
        let m = g.merge(1, 9).unwrap();
        assert_eq!(
            m,
            Merged {
                group: id,
                absorbed: None
            }
        );
        assert_eq!(members(&g, id), vec![9, 1, 2]);
        assert_eq!(g.get(id).unwrap().active(), 1);
    }

    #[test]
    fn merging_two_groups_absorbs_the_dragged_one() {
        let mut g = Groups::new();
        let a = g.merge(2, 1).unwrap().group;
        let b = g.merge(4, 3).unwrap().group;
        let m = g.merge(4, 1).unwrap();
        assert_eq!(
            m,
            Merged {
                group: a,
                absorbed: Some(b)
            }
        );
        assert_eq!(members(&g, a), vec![1, 2, 3, 4]);
        assert!(g.get(b).is_none());
        assert_eq!(g.group_of(3), Some(a));
    }

    #[test]
    fn merge_rejects_nonsense() {
        let mut g = Groups::new();
        assert_eq!(g.merge(1, 1), Err(MergeError::SameWindow));
        g.merge(2, 1).unwrap();
        assert_eq!(g.merge(1, 2), Err(MergeError::AlreadyTogether));
    }

    #[test]
    fn removing_the_active_tab_activates_its_right_neighbour() {
        let mut g = Groups::new();
        let id = g.merge(2, 1).unwrap().group;
        g.merge(3, 1).unwrap();
        g.activate(2);
        assert_eq!(
            g.remove(2),
            Removed::Remains {
                group: id,
                active: 3,
                active_changed: true
            }
        );
        assert_eq!(members(&g, id), vec![1, 3]);
    }

    #[test]
    fn removing_the_last_active_tab_activates_its_left_neighbour() {
        let mut g = Groups::new();
        let id = g.merge(2, 1).unwrap().group;
        g.merge(3, 1).unwrap();
        assert_eq!(
            g.remove(3),
            Removed::Remains {
                group: id,
                active: 2,
                active_changed: true
            }
        );
    }

    #[test]
    fn removing_an_inactive_tab_keeps_the_active_window() {
        let mut g = Groups::new();
        let id = g.merge(2, 1).unwrap().group;
        g.merge(3, 1).unwrap();
        assert_eq!(
            g.remove(1),
            Removed::Remains {
                group: id,
                active: 3,
                active_changed: false
            }
        );
        assert_eq!(g.get(id).unwrap().active_index(), 1);
    }

    #[test]
    fn removing_down_to_one_window_dissolves() {
        let mut g = Groups::new();
        let id = g.merge(2, 1).unwrap().group;
        assert_eq!(
            g.remove(1),
            Removed::Dissolved {
                group: id,
                survivor: 2
            }
        );
        assert!(g.ids().is_empty());
        assert_eq!(g.group_of(2), None);
        assert_eq!(g.remove(2), Removed::NotGrouped);
    }

    #[test]
    fn neighbour_wraps_both_ways() {
        let mut g = Groups::new();
        let id = g.merge(2, 1).unwrap().group;
        g.merge(3, 1).unwrap();
        g.activate(1);
        assert_eq!(g.neighbour(id, true), Some(2));
        assert_eq!(g.neighbour(id, false), Some(3));
    }

    #[test]
    fn activate_reports_only_changes() {
        let mut g = Groups::new();
        let id = g.merge(2, 1).unwrap().group;
        assert_eq!(g.activate(2), None);
        assert_eq!(g.activate(1), Some(id));
        assert_eq!(g.activate(7), None);
    }

    #[test]
    fn move_tab_keeps_the_active_window() {
        let mut g = Groups::new();
        let id = g.merge(2, 1).unwrap().group;
        g.merge(3, 1).unwrap();
        g.activate(1);
        assert!(g.move_tab(id, 0, 2));
        assert_eq!(members(&g, id), vec![2, 3, 1]);
        assert_eq!(g.get(id).unwrap().active(), 1);
        assert!(!g.move_tab(id, 0, 5));
    }

    #[test]
    fn dissolve_returns_members_in_tab_order() {
        let mut g = Groups::new();
        let id = g.merge(2, 1).unwrap().group;
        g.merge(3, 1).unwrap();
        assert_eq!(g.dissolve(id), vec![1, 2, 3]);
        assert!(g.ids().is_empty());
        assert_eq!(g.group_of(1), None);
    }

    #[test]
    fn inactive_lists_everyone_but_the_active_window() {
        let mut g = Groups::new();
        let id = g.merge(2, 1).unwrap().group;
        g.merge(3, 1).unwrap();
        let inactive: Vec<_> = g.get(id).unwrap().inactive().collect();
        assert_eq!(inactive, vec![1, 2]);
    }
}
