//! Self-update bookkeeping: what this build is, what the newest build of `main` is, how many
//! commits apart they are, and which network job to run next.
//!
//! CI numbers every build of `main` with its commit count (`git rev-list --count HEAD`) and
//! publishes it as the latest GitHub release, with a small `manifest.txt` beside the executables.
//! When this build's commit is an ancestor of the newest one (always true on `main` unless its
//! history is rewritten), the difference between the two numbers is exactly the number of
//! commits between them, so "commits behind" needs only that manifest.

use std::collections::BTreeMap;

pub const MANIFEST_NAME: &str = "manifest.txt";

/// Release asset suffix and PE machine type for the executable being compiled.
pub const ARCH: &str = if cfg!(target_arch = "aarch64") {
    "arm64"
} else if cfg!(target_arch = "x86_64") {
    "x64"
} else {
    "x86"
};
pub const PE_MACHINE: u16 = if cfg!(target_arch = "aarch64") {
    0xAA64
} else if cfg!(target_arch = "x86_64") {
    0x8664
} else {
    0x014C
};

const DEFAULT_REPO: &str = "Trillx/grouply";

/// Identity baked in at compile time by CI. Local builds are build 0 and never update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildInfo {
    pub build: u32,
    pub commit: &'static str,
    pub repo: &'static str,
}

impl BuildInfo {
    pub fn current() -> Self {
        Self {
            build: option_env!("GROUPLY_BUILD")
                .and_then(|b| b.parse().ok())
                .unwrap_or(0),
            commit: option_env!("GROUPLY_COMMIT").unwrap_or(""),
            repo: option_env!("GROUPLY_REPO").unwrap_or(DEFAULT_REPO),
        }
    }

    pub fn is_release(&self) -> bool {
        self.build > 0
    }

    pub fn label(&self) -> String {
        if self.is_release() {
            format!("build {} ({})", self.build, short(self.commit))
        } else {
            "development build".into()
        }
    }

    pub fn manifest_url(&self) -> String {
        format!(
            "https://github.com/{}/releases/latest/download/{MANIFEST_NAME}",
            self.repo
        )
    }
}

fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub url: String,
    /// Lowercase hex.
    pub sha256: String,
}

/// The newest build of `main`, as described by its release's `manifest.txt`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub build: u32,
    pub commit: String,
    assets: BTreeMap<String, Asset>,
}

impl Manifest {
    /// Parses `key=value` lines: `build`, `commit`, and `<arch>.url` / `<arch>.sha256` for each
    /// executable. Blank lines, `#` comments and unknown keys are ignored.
    pub fn parse(text: &str) -> Option<Self> {
        let mut build = None;
        let mut commit = None;
        let mut urls = BTreeMap::new();
        let mut hashes = BTreeMap::new();
        for line in text.lines().map(str::trim) {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "build" => build = value.parse::<u32>().ok(),
                "commit" => commit = Some(value.to_owned()),
                _ => {
                    if let Some(arch) = key.strip_suffix(".url") {
                        urls.insert(arch.to_owned(), value.to_owned());
                    } else if let Some(arch) = key.strip_suffix(".sha256") {
                        hashes.insert(arch.to_owned(), value.to_ascii_lowercase());
                    }
                }
            }
        }
        let assets = urls
            .into_iter()
            .filter_map(|(arch, url)| {
                let sha256 = hashes.remove(&arch)?;
                let valid = url.starts_with("https://")
                    && sha256.len() == 64
                    && sha256.bytes().all(|b| b.is_ascii_hexdigit());
                valid.then_some((arch, Asset { url, sha256 }))
            })
            .collect();
        Some(Self {
            build: build.filter(|&b| b > 0)?,
            commit: commit.filter(|c| !c.is_empty())?,
            assets,
        })
    }

    pub fn asset(&self, arch: &str) -> Option<&Asset> {
        self.assets.get(arch)
    }
}

/// "1 commit behind main", "3 commits behind main".
pub fn behind_text(commits: u32) -> String {
    let noun = if commits == 1 { "commit" } else { "commits" };
    format!("{commits} {noun} behind main")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Disabled,
    NotChecked,
    Checking,
    UpToDate,
    Behind(u32),
    Downloading,
    CheckFailed(String),
}

impl Status {
    pub fn describe(&self) -> String {
        match self {
            Status::Disabled => "Updates off (development build)".into(),
            Status::NotChecked => "Not checked for updates yet".into(),
            Status::Checking => "Checking for updates…".into(),
            Status::UpToDate => "Up to date with main".into(),
            Status::Behind(n) => behind_text(*n),
            Status::Downloading => "Downloading update…".into(),
            Status::CheckFailed(error) => format!("Couldn't check for updates: {error}"),
        }
    }
}

/// Network work for the platform layer to run off the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    Check { url: String },
    Download(Asset),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Busy {
    Checking,
    Downloading,
}

/// Update state for one running instance. It decides what to fetch; the caller fetches it and
/// reports back through `checked` and `download_finished`.
#[derive(Debug)]
pub struct Updater {
    build: BuildInfo,
    arch: &'static str,
    busy: Option<Busy>,
    latest: Option<Manifest>,
    error: Option<String>,
    /// The newest build already announced, so each one is announced once.
    announced: u32,
}

impl Updater {
    pub fn new(build: BuildInfo, arch: &'static str) -> Self {
        Self {
            build,
            arch,
            busy: None,
            latest: None,
            error: None,
            announced: build.build,
        }
    }

    pub fn build(&self) -> BuildInfo {
        self.build
    }

    pub fn can_check(&self) -> bool {
        self.build.is_release() && self.busy.is_none()
    }

    /// Commits between this build and the newest one that has an executable for this machine.
    pub fn behind(&self) -> u32 {
        self.latest
            .as_ref()
            .filter(|m| m.asset(self.arch).is_some())
            .map_or(0, |m| m.build.saturating_sub(self.build.build))
    }

    pub fn can_install(&self) -> bool {
        self.busy.is_none() && self.behind() > 0
    }

    pub fn begin_check(&mut self) -> Option<Job> {
        if !self.can_check() {
            return None;
        }
        self.busy = Some(Busy::Checking);
        Some(Job::Check {
            url: self.build.manifest_url(),
        })
    }

    /// Records a check's result. Returns the commit count to announce when it found a newer build
    /// than any announced before.
    pub fn checked(&mut self, result: Result<Manifest, String>) -> Option<u32> {
        if self.busy == Some(Busy::Checking) {
            self.busy = None;
        }
        match result {
            Ok(manifest) => {
                self.error = None;
                self.latest = Some(manifest);
            }
            Err(error) => {
                // Keep what an earlier check found; a failed check says nothing new.
                self.error = Some(error);
                return None;
            }
        }
        let behind = self.behind();
        let newest = self.latest.as_ref().map_or(0, |m| m.build);
        if behind > 0 && newest > self.announced {
            self.announced = newest;
            return Some(behind);
        }
        None
    }

    pub fn begin_install(&mut self) -> Option<Job> {
        if !self.can_install() {
            return None;
        }
        let asset = self.latest.as_ref()?.asset(self.arch)?.clone();
        self.busy = Some(Busy::Downloading);
        Some(Job::Download(asset))
    }

    pub fn download_finished(&mut self) {
        if self.busy == Some(Busy::Downloading) {
            self.busy = None;
        }
    }

    pub fn status(&self) -> Status {
        if !self.build.is_release() {
            return Status::Disabled;
        }
        match self.busy {
            Some(Busy::Checking) => return Status::Checking,
            Some(Busy::Downloading) => return Status::Downloading,
            None => {}
        }
        if self.latest.is_some() {
            return match self.behind() {
                0 => Status::UpToDate,
                n => Status::Behind(n),
            };
        }
        match &self.error {
            Some(error) => Status::CheckFailed(error.clone()),
            None => Status::NotChecked,
        }
    }

    /// The tray tooltip, e.g. "Grouply build 42 (abc1234): 3 commits behind main".
    pub fn tooltip(&self) -> String {
        format!(
            "Grouply {}: {}",
            self.build.label(),
            self.status().describe()
        )
    }
}

/// Splits an `https://host/path` URL into host and path. Ports and credentials aren't supported.
pub fn split_https_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, "/"),
    };
    let valid = !host.is_empty() && !host.contains(['@', ':']);
    valid.then_some((host, path))
}

/// The machine type in a PE image's header, or None if `bytes` isn't a PE image.
pub fn pe_machine(bytes: &[u8]) -> Option<u16> {
    if bytes.get(..2)? != b"MZ" {
        return None;
    }
    let offset = u32::from_le_bytes(bytes.get(0x3C..0x40)?.try_into().ok()?) as usize;
    if bytes.get(offset..offset.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    let machine = bytes.get(offset + 4..offset + 6)?;
    Some(u16::from_le_bytes([machine[0], machine[1]]))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12";

    fn release(build: u32) -> BuildInfo {
        BuildInfo {
            build,
            commit: "0123456789abcdef",
            repo: "someone/grouply",
        }
    }

    fn manifest(build: u32) -> Manifest {
        Manifest::parse(&format!(
            "build={build}\ncommit=fedcba9876543210\narm64.url=https://example.com/a.exe\narm64.sha256={HASH}\n"
        ))
        .unwrap()
    }

    #[test]
    fn parses_manifest() {
        let text = format!(
            "# Grouply\nbuild = 42\ncommit=abc\n\narm64.url=https://h/a.exe\narm64.sha256={}\nx64.url=https://h/b.exe\nx64.sha256={HASH}\nnoise\nfuture=1\n",
            HASH.to_uppercase()
        );
        let m = Manifest::parse(&text).unwrap();
        assert_eq!((m.build, m.commit.as_str()), (42, "abc"));
        assert_eq!(
            m.asset("arm64"),
            Some(&Asset {
                url: "https://h/a.exe".into(),
                sha256: HASH.into()
            })
        );
        assert!(m.asset("x64").is_some());
        assert!(m.asset("x86").is_none());
    }

    #[test]
    fn rejects_incomplete_manifests_and_assets() {
        assert!(Manifest::parse("commit=abc").is_none());
        assert!(Manifest::parse("build=0\ncommit=abc").is_none());
        assert!(Manifest::parse("build=3").is_none());
        let m = Manifest::parse(&format!(
            "build=3\ncommit=abc\narm64.url=http://h/a.exe\narm64.sha256={HASH}\nx64.url=https://h/b.exe\nx64.sha256=123"
        ))
        .unwrap();
        assert!(m.asset("arm64").is_none(), "plain http is refused");
        assert!(m.asset("x64").is_none(), "hash must be 64 hex digits");
    }

    #[test]
    fn development_builds_never_update() {
        let mut u = Updater::new(release(0), "arm64");
        assert_eq!(u.begin_check(), None);
        assert_eq!(u.status(), Status::Disabled);
        assert_eq!(u.build().label(), "development build");
    }

    #[test]
    fn counts_commits_behind_and_announces_each_build_once() {
        let mut u = Updater::new(release(40), "arm64");
        assert_eq!(u.status(), Status::NotChecked);
        assert_eq!(
            u.begin_check(),
            Some(Job::Check {
                url: "https://github.com/someone/grouply/releases/latest/download/manifest.txt"
                    .into()
            })
        );
        assert_eq!(u.status(), Status::Checking);
        assert_eq!(u.begin_check(), None, "one check at a time");
        assert_eq!(u.checked(Ok(manifest(43))), Some(3));
        assert_eq!(u.status(), Status::Behind(3));
        assert_eq!(
            u.tooltip(),
            "Grouply build 40 (0123456): 3 commits behind main"
        );

        u.begin_check();
        assert_eq!(u.checked(Ok(manifest(43))), None, "already announced");
        u.begin_check();
        assert_eq!(u.checked(Ok(manifest(44))), Some(4));
    }

    #[test]
    fn failed_check_keeps_what_was_known() {
        let mut u = Updater::new(release(40), "arm64");
        u.begin_check();
        assert_eq!(u.checked(Err("offline".into())), None);
        assert_eq!(u.status(), Status::CheckFailed("offline".into()));
        u.begin_check();
        u.checked(Ok(manifest(41)));
        u.begin_check();
        u.checked(Err("offline".into()));
        assert_eq!(u.status(), Status::Behind(1));
    }

    #[test]
    fn up_to_date_or_ahead_or_missing_arch_means_nothing_to_install() {
        for (ours, latest, arch) in [(43, 43, "arm64"), (44, 43, "arm64"), (40, 43, "x64")] {
            let mut u = Updater::new(release(ours), arch);
            u.begin_check();
            assert_eq!(u.checked(Ok(manifest(latest))), None);
            assert_eq!(u.status(), Status::UpToDate);
            assert_eq!(u.begin_install(), None);
        }
    }

    #[test]
    fn installs_the_asset_for_this_machine() {
        let mut u = Updater::new(release(40), "arm64");
        u.begin_check();
        u.checked(Ok(manifest(41)));
        let job = u.begin_install().unwrap();
        assert_eq!(
            job,
            Job::Download(Asset {
                url: "https://example.com/a.exe".into(),
                sha256: HASH.into()
            })
        );
        assert_eq!(u.status(), Status::Downloading);
        assert_eq!(u.begin_install(), None);
        assert_eq!(u.begin_check(), None);
        u.download_finished();
        assert_eq!(u.status(), Status::Behind(1));
    }

    #[test]
    fn behind_text_pluralises() {
        assert_eq!(behind_text(1), "1 commit behind main");
        assert_eq!(behind_text(12), "12 commits behind main");
    }

    #[test]
    fn splits_urls() {
        assert_eq!(
            split_https_url("https://github.com/o/r/releases/latest/download/manifest.txt"),
            Some(("github.com", "/o/r/releases/latest/download/manifest.txt"))
        );
        assert_eq!(split_https_url("https://h"), Some(("h", "/")));
        assert_eq!(split_https_url("http://h/x"), None);
        assert_eq!(split_https_url("https://u@h/x"), None);
        assert_eq!(split_https_url("https://h:8443/x"), None);
        assert_eq!(split_https_url("https:///x"), None);
    }

    #[test]
    fn reads_pe_machine() {
        let mut image = vec![0u8; 0x48];
        image[..2].copy_from_slice(b"MZ");
        image[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        image[0x40..0x44].copy_from_slice(b"PE\0\0");
        image[0x44..0x46].copy_from_slice(&0xAA64u16.to_le_bytes());
        assert_eq!(pe_machine(&image), Some(0xAA64));
        assert_eq!(pe_machine(&image[..0x45]), None);
        assert_eq!(pe_machine(b"PK\x03\x04"), None);
        image[0x40] = b'X';
        assert_eq!(pe_machine(&image), None);
    }

    #[test]
    fn hex_encodes() {
        assert_eq!(hex(&[0x00, 0xab, 0x7f]), "00ab7f");
    }
}
