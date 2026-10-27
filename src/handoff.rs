//! Carries groups across a restart, such as after an update, on the new instance's command line.
//!
//! Window handles stay valid across processes in a session, so the new instance can regroup the
//! same windows. Groups are separated by `;` and members by `,`, as hex handles in tab order, with
//! `*` marking the active tab: `*1a2b,3c4d;5e6f,*7a8b`.

const UPDATED_FLAG: &str = "--updated";
const GROUPS_FLAG: &str = "--groups=";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedGroup {
    pub members: Vec<isize>,
    pub active: usize,
}

pub fn encode(groups: &[SavedGroup]) -> String {
    groups
        .iter()
        .map(|g| {
            g.members
                .iter()
                .enumerate()
                .map(|(i, &h)| {
                    let mark = if i == g.active { "*" } else { "" };
                    format!("{mark}{:x}", h as usize)
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// Parses `encode`'s output, skipping anything malformed and groups of fewer than two windows.
pub fn decode(text: &str) -> Vec<SavedGroup> {
    text.split(';')
        .filter_map(|group| {
            let mut active = 0;
            let mut members = Vec::new();
            for (i, item) in group.split(',').enumerate() {
                let hex = match item.strip_prefix('*') {
                    Some(rest) => {
                        active = i;
                        rest
                    }
                    None => item,
                };
                members.push(usize::from_str_radix(hex, 16).ok().filter(|&h| h != 0)? as isize);
            }
            (members.len() >= 2).then_some(SavedGroup { members, active })
        })
        .collect()
}

/// How this instance was started.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    /// Started by the updater: the previous instance is still exiting.
    pub updated: bool,
    pub groups: Vec<SavedGroup>,
}

impl Launch {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Self {
        let mut launch = Self::default();
        for arg in args {
            if arg == UPDATED_FLAG {
                launch.updated = true;
            } else if let Some(spec) = arg.strip_prefix(GROUPS_FLAG) {
                launch.groups = decode(spec);
            }
        }
        launch
    }

    /// Arguments that start the next instance with `groups`.
    pub fn args_for(groups: &[SavedGroup]) -> Vec<String> {
        let mut args = vec![UPDATED_FLAG.to_owned()];
        if !groups.is_empty() {
            args.push(format!("{GROUPS_FLAG}{}", encode(groups)));
        }
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(members: &[isize], active: usize) -> SavedGroup {
        SavedGroup {
            members: members.to_vec(),
            active,
        }
    }

    #[test]
    fn round_trips() {
        let groups = vec![group(&[0x1a2b, 0x3c4d], 0), group(&[0x5e, 0x6f, 0x70], 2)];
        let text = encode(&groups);
        assert_eq!(text, "*1a2b,3c4d;5e,6f,*70");
        assert_eq!(decode(&text), groups);
    }

    #[test]
    fn skips_malformed_groups() {
        assert_eq!(
            decode("*1a,zz;5,*6;7;;0,*8"),
            vec![group(&[5, 6], 1)],
            "bad hex, single members, empty groups and null handles are dropped"
        );
        assert!(decode("").is_empty());
    }

    #[test]
    fn parses_launch_arguments() {
        let args = Launch::args_for(&[group(&[1, 2], 1)]);
        assert_eq!(args, ["--updated", "--groups=1,*2"]);
        let launch = Launch::parse(args);
        assert!(launch.updated);
        assert_eq!(launch.groups, vec![group(&[1, 2], 1)]);

        assert_eq!(Launch::parse(Vec::new()), Launch::default());
        assert_eq!(Launch::args_for(&[]), ["--updated"]);
    }
}
