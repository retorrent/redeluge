// SPDX-License-Identifier: GPL-3.0-or-later
//! Torrents whose files live on a mounted archive: S3 through rclone, a NAS.
//!
//! The machine has a good link and a small disk, and most torrents go quiet
//! once they finish. So a label can send its torrents to an archive target
//! once they have been idle long enough, and they keep seeding from there.
//! libtorrent sees an ordinary directory; the mount does the rest.
//!
//! The part that needs care is the mount going away. A FUSE mount that dies
//! answers `ENOTCONN`, one that is unmounted is an empty directory, and one
//! whose remote is slow just hangs. Left alone, every torrent on it goes to
//! Error, and on a restart libtorrent finds no files, rechecks and starts
//! downloading again into the empty mount point. So:
//!
//! * Each target is probed every pass, with a timeout, from a thread that may
//!   be left behind if the mount hangs.
//! * While a target is offline its torrents are paused, an error they hit is
//!   cleared, and nothing removes or moves their files.
//! * When it comes back they resume where they were, without a recheck: a
//!   recheck reads the whole archive back from the remote.
//! * At startup, torrents on an offline target are restored paused and are not
//!   verified against the disk, so their progress survives.
//!
//! Where a torrent's files are is not stored anywhere: it is its save path,
//! under one target's path or none.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// How long a probe may take before the mount counts as hung.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a torrent that hit an error on a target that is up waits before
/// it is tried again. A remote that timed out once is usually back by then.
pub const RETRY_AFTER: f64 = 300.0;

/// One place torrents can be archived to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    /// What a label names it by.
    pub name: String,
    /// Where it is mounted, or a directory inside the mount.
    pub path: String,
    /// rclone's remote control, `http://127.0.0.1:5572`, when the mount runs
    /// with `--rc`. Optional: without it there is no "syncing" state, only
    /// whether the mount answers.
    #[serde(default)]
    pub rc_url: String,
}

/// The `archive` key of `core.conf`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub targets: Vec<Target>,
}

impl Settings {
    pub fn from_config(value: Option<&Json>) -> Self {
        match value {
            Some(value) => {
                serde_json::from_value(super::without_nulls(value)).unwrap_or_else(|err| {
                    super::warn_malformed("archive", &err.to_string());
                    Self::default()
                })
            }
            None => Self::default(),
        }
    }

    pub fn default_json() -> Json {
        serde_json::to_value(Self::default()).expect("the defaults serialise")
    }

    /// The targets that can be used: a name and an absolute path each.
    pub fn usable(&self) -> Vec<Target> {
        self.targets
            .iter()
            .filter(|target| !target.name.trim().is_empty())
            .filter(|target| Path::new(target.path.trim()).is_absolute())
            .map(|target| Target {
                name: target.name.trim().to_owned(),
                path: target.path.trim().trim_end_matches('/').to_owned(),
                rc_url: target.rc_url.trim().trim_end_matches('/').to_owned(),
            })
            .collect()
    }
}

/// An rc URL fit for a log line: `http://user:pass@host` loses the
/// credentials, which is how rclone's `--rc-user` / `--rc-pass` are given.
pub fn redact(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(scheme), Some(at)) if at > scheme => {
            format!("{}***@{}", &url[..scheme + 3], &url[at + 1..])
        }
        _ => url.to_owned(),
    }
}

/// Whether `path` is `root` or inside it, by whole components.
pub fn is_under(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    let path = path.trim_end_matches('/');
    !root.is_empty() && (path == root || path.starts_with(&format!("{root}/")))
}

/// The target a path is on, the most specific one when targets nest.
pub fn target_of<'a>(path: &str, targets: &'a [Target]) -> Option<&'a Target> {
    targets
        .iter()
        .filter(|target| is_under(path, &target.path))
        .max_by_key(|target| target.path.len())
}

/// Where a torrent's files are, as far as archiving goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveState {
    /// Being moved onto a target.
    Archiving,
    /// On a target, and rclone is still uploading some of its files.
    Syncing,
    /// On a target that answers, nothing left to upload.
    Archived,
    /// On a target that does not answer, or waiting to retry after an error
    /// there. Paused until it does.
    Offline,
}

impl ArchiveState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Archiving => "archiving",
            Self::Syncing => "syncing",
            Self::Archived => "archived",
            Self::Offline => "offline",
        }
    }
}

/// Filesystem types an archive can be on: FUSE (rclone, sshfs…) and the
/// network filesystems. Anything else under a target's path means the mount
/// is not there and what is visible is the directory it would cover.
// ponytail: a target on a second local disk is never "online"; archive targets
// are remote by design. A per-target expected type if that is ever wanted.
const REMOTE_FILESYSTEMS: &[&str] = &[
    "fuse",
    "nfs",
    "cifs",
    "smb",
    "9p",
    "ceph",
    "glusterfs",
    "afs",
    "davfs",
    "virtiofs",
];

/// The type of the filesystem that holds `path`, from a `mountinfo` listing:
/// the mount with the longest mount point that is `path` or above it.
fn covering_fstype(mountinfo: &str, path: &str) -> Option<String> {
    mountinfo
        .lines()
        .filter_map(|line| {
            let (before, after) = line.split_once(" - ")?;
            let mount_point = before.split(' ').nth(4)?.replace("\\040", " ");
            let fstype = after.split(' ').next()?;
            Some((mount_point, fstype.to_owned()))
        })
        .filter(|(mount_point, _)| mount_point == "/" || is_under(path, mount_point))
        .max_by_key(|(mount_point, _)| mount_point.len())
        .map(|(_, fstype)| fstype)
}

/// Whether a filesystem type is one an archive can be on.
fn is_remote(fstype: &str) -> bool {
    // By prefix: `fuse.rclone`, `nfs4`, `smb3` are all spelled that way.
    REMOTE_FILESYSTEMS
        .iter()
        .any(|remote| fstype.starts_with(remote))
}

/// Whether a target answers. Blocking, and it can hang: see [`probe`].
///
/// Answering means the path is on a FUSE or network mount and can be listed.
/// The type matters, not just "is there a mount": in Docker the target is
/// usually a bind mount itself, so there always is one, and when rclone is
/// down what is left is that bind — an empty local directory that lists fine
/// and holds none of the files. A crashed FUSE mount is still listed as fuse
/// but cannot be read, which the listing catches.
fn probe_blocking(path: &str) -> bool {
    let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    if !covering_fstype(&mountinfo, path).is_some_and(|fstype| is_remote(&fstype)) {
        return false;
    }
    std::fs::read_dir(path).is_ok()
}

/// Whether a target answers, within [`PROBE_TIMEOUT`].
///
/// The probe runs on a blocking thread that is abandoned if it does not come
/// back: a hung FUSE call cannot be interrupted, and the next pass must not
/// wait on it.
pub async fn probe(path: String) -> bool {
    let task = tokio::task::spawn_blocking(move || probe_blocking(&path));
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, task).await,
        Ok(Ok(true))
    )
}

/// One call to rclone's remote control. `None` when it did not answer.
async fn rc(client: &reqwest::Client, rc_url: &str, method: &str, body: Json) -> Option<Json> {
    let answer = client
        .post(format!("{rc_url}/{method}"))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    serde_json::from_slice(&answer).ok()
}

/// What rclone has yet to upload, as paths relative to the remote's root.
///
/// `None` when rclone did not answer, which is "unknown", not "nothing".
pub async fn upload_queue(client: &reqwest::Client, rc_url: &str) -> Option<Vec<String>> {
    let answer = rc(
        client,
        rc_url,
        "vfs/queue",
        Json::Object(Default::default()),
    )
    .await?;
    Some(
        answer
            .get("queue")
            .and_then(Json::as_array)
            .map(|queue| {
                queue
                    .iter()
                    .filter_map(|item| item.get("name").and_then(Json::as_str))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// What rclone's VFS cache holds on the local disk, and how many uploads are
/// still to go, from `vfs/stats`. `None` when rclone did not answer.
pub async fn cache_stats(client: &reqwest::Client, rc_url: &str) -> Option<(i64, i64)> {
    let answer = rc(
        client,
        rc_url,
        "vfs/stats",
        Json::Object(Default::default()),
    )
    .await?;
    let cache = answer.get("diskCache")?;
    let number = |key: &str| cache.get(key).and_then(Json::as_i64).unwrap_or(0);
    Some((
        number("bytesUsed"),
        number("uploadsInProgress") + number("uploadsQueued"),
    ))
}

/// The remote settings it is safe to show. Everything else in a remote's
/// configuration — keys, secrets, tokens, passwords — stays in rclone.
const SHOWN_REMOTE_KEYS: &[&str] = &[
    "type",
    "provider",
    "endpoint",
    "region",
    "location_constraint",
    "storage_class",
];

/// How rclone is set up behind a target, as far as its remote control says:
/// version, the remote and bucket it mounts, the remote's public settings and
/// the VFS cache options. An empty object when there is no rc or it is down.
pub async fn describe(client: &reqwest::Client, rc_url: &str) -> Json {
    let mut out = serde_json::Map::new();
    let empty = || Json::Object(Default::default());
    let Some(version) = rc(client, rc_url, "core/version", empty()).await else {
        return Json::Object(out);
    };
    out.insert(
        "version".into(),
        version.get("version").cloned().unwrap_or(Json::Null),
    );

    // `remote:bucket/path`, as the mount was given it.
    let mounted = rc(client, rc_url, "vfs/list", empty())
        .await
        .and_then(|list| {
            list.get("vfses")?
                .as_array()?
                .first()?
                .as_str()
                .map(str::to_owned)
        });
    if let Some(mounted) = mounted {
        let (remote, bucket) = mounted.split_once(':').unwrap_or((&mounted, ""));
        out.insert("remote".into(), Json::from(remote));
        out.insert("bucket".into(), Json::from(bucket));
        if let Some(config) = rc(
            client,
            rc_url,
            "config/get",
            serde_json::json!({ "name": remote }),
        )
        .await
        {
            for key in SHOWN_REMOTE_KEYS {
                if let Some(value) = config.get(*key) {
                    out.insert((*key).into(), value.clone());
                }
            }
        }
    }

    if let Some(options) = rc(client, rc_url, "options/get", empty()).await {
        let vfs = options.get("vfs");
        let get = |key: &str| vfs.and_then(|vfs| vfs.get(key)).cloned();
        let seconds = |key: &str| {
            get(key)
                .and_then(|value| value.as_i64())
                .map(|nanos| Json::from(nanos / 1_000_000_000))
        };
        if let Some(mode) = get("CacheMode") {
            out.insert("cache_mode".into(), mode);
        }
        if let Some(size) = get("CacheMaxSize") {
            out.insert("cache_max_size".into(), size);
        }
        if let Some(write_back) = seconds("WriteBack") {
            out.insert("write_back".into(), write_back);
        }
        if let Some(age) = seconds("CacheMaxAge") {
            out.insert("cache_max_age".into(), age);
        }
    }
    Json::Object(out)
}

/// Whether any queued upload belongs to a torrent's files.
///
/// `relative` is the torrent's top-level file or directory, relative to its
/// target's path. The queue is relative to the remote's root, which may be
/// above the target's path, so this matches on whole components anywhere.
// ponytail: matches by path components, so two targets on one rclone holding
// the same relative name would share a "syncing"; per-target prefixes if that
// ever happens.
pub fn is_syncing(relative: &str, queue: &[String]) -> bool {
    let relative = relative.trim_matches('/');
    if relative.is_empty() {
        return !queue.is_empty();
    }
    queue.iter().any(|name| {
        let name = name.trim_matches('/');
        name == relative
            || name.starts_with(&format!("{relative}/"))
            || name.ends_with(&format!("/{relative}"))
            || name.contains(&format!("/{relative}/"))
    })
}

/// How long a finished torrent has gone without moving a byte either way.
///
/// The time since its last transfer, or since it finished when it never had
/// one. `None` when neither is known: an archive move waits rather than guess.
pub fn idle_seconds(
    since_download: i64,
    since_upload: i64,
    finished_at: f64,
    now: f64,
) -> Option<f64> {
    let last = [since_download, since_upload]
        .into_iter()
        .filter(|seconds| *seconds >= 0)
        .min();
    match last {
        Some(seconds) => Some(seconds as f64),
        None if finished_at > 0.0 => Some(now - finished_at),
        None => None,
    }
}

/// What one torrent on a target should have done to it this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// Pause it: its target is offline, or it hit an error there.
    Pause,
    /// Let it go again: its target answers and its wait is over.
    Release,
    /// Leave it as it is.
    Leave,
}

/// The offline rule for one torrent on a target.
///
/// `held` is whether this rule paused it; `paused` whether it is paused at
/// all; `errored` whether libtorrent reports an error for it; `retry_at` when a
/// torrent paused after an error may be tried again.
pub fn hold(
    online: bool,
    held: bool,
    paused: bool,
    errored: bool,
    retry_at: f64,
    now: f64,
) -> Hold {
    if !online {
        // A torrent somebody paused stays theirs; one in error is taken too,
        // so the error is cleared and it comes back on its own.
        if held || (paused && !errored) {
            return Hold::Leave;
        }
        return Hold::Pause;
    }
    if errored {
        // The target answers and the torrent still failed: a read that timed
        // out, usually. Hold it and try again later rather than leave it in
        // Error for somebody to find.
        return if held && now < retry_at {
            Hold::Leave
        } else {
            Hold::Pause
        };
    }
    if held && now >= retry_at {
        return Hold::Release;
    }
    Hold::Leave
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn targets() -> Vec<Target> {
        Settings::from_config(Some(&json!({"targets": [
            {"name": "s3", "path": "/mnt/s3/"},
            {"name": "deep", "path": "/mnt/s3/deep", "rc_url": "http://127.0.0.1:5572/"},
            {"name": "", "path": "/mnt/nameless"},
            {"name": "relative", "path": "mnt/rel"},
        ]})))
        .usable()
    }

    #[test]
    fn rc_credentials_stay_out_of_the_log() {
        assert_eq!(
            redact("http://lab:secret@rclone:5572"),
            "http://***@rclone:5572"
        );
        assert_eq!(redact("http://127.0.0.1:5572"), "http://127.0.0.1:5572");
    }

    #[test]
    fn only_named_absolute_targets_are_used() {
        let targets = targets();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].path, "/mnt/s3");
        assert_eq!(targets[1].rc_url, "http://127.0.0.1:5572");
    }

    #[test]
    fn a_path_belongs_to_the_most_specific_target_by_whole_components() {
        let targets = targets();
        assert_eq!(target_of("/mnt/s3", &targets).unwrap().name, "s3");
        assert_eq!(target_of("/mnt/s3/films", &targets).unwrap().name, "s3");
        assert_eq!(target_of("/mnt/s3/deep/x", &targets).unwrap().name, "deep");
        assert!(target_of("/mnt/s3x/films", &targets).is_none());
        assert!(target_of("/downloads", &targets).is_none());
    }

    #[test]
    fn syncing_matches_the_torrents_files_wherever_the_remote_root_is() {
        let queue = vec!["archive/Ubuntu ISO/ubuntu.iso".to_owned()];
        assert!(is_syncing("Ubuntu ISO", &queue));
        assert!(!is_syncing("Ubuntu", &queue));
        assert!(is_syncing("ubuntu.iso", &queue));
        assert!(!is_syncing("Debian", &queue));
        assert!(!is_syncing("Debian", &[]));
    }

    #[test]
    fn idle_is_the_latest_transfer_or_the_finish() {
        assert_eq!(idle_seconds(100, 30, 0.0, 1000.0), Some(30.0));
        assert_eq!(idle_seconds(-1, 50, 0.0, 1000.0), Some(50.0));
        assert_eq!(idle_seconds(-1, -1, 400.0, 1000.0), Some(600.0));
        assert_eq!(idle_seconds(-1, -1, 0.0, 1000.0), None);
    }

    #[test]
    fn an_offline_target_holds_its_torrents_but_not_ones_somebody_paused() {
        assert_eq!(hold(false, false, false, false, 0.0, 0.0), Hold::Pause);
        assert_eq!(hold(false, false, false, true, 0.0, 0.0), Hold::Pause);
        assert_eq!(hold(false, false, true, true, 0.0, 0.0), Hold::Pause);
        assert_eq!(hold(false, false, true, false, 0.0, 0.0), Hold::Leave);
        assert_eq!(hold(false, true, true, false, 0.0, 0.0), Hold::Leave);
    }

    #[test]
    fn a_target_coming_back_releases_what_it_held_and_only_that() {
        assert_eq!(hold(true, true, true, false, 0.0, 10.0), Hold::Release);
        assert_eq!(hold(true, false, true, false, 0.0, 10.0), Hold::Leave);
        assert_eq!(hold(true, false, false, false, 0.0, 10.0), Hold::Leave);
    }

    #[test]
    fn an_error_on_a_target_that_answers_is_held_and_retried() {
        assert_eq!(hold(true, false, false, true, 0.0, 10.0), Hold::Pause);
        assert_eq!(hold(true, true, true, false, 100.0, 10.0), Hold::Leave);
        assert_eq!(hold(true, true, true, false, 100.0, 100.0), Hold::Release);
    }

    #[test]
    fn a_bind_mount_left_behind_is_not_the_archive() {
        // rclone's container binds the host directory, and redeluge's binds
        // it again: with rclone up the path is under fuse.rclone, with rclone
        // down it is under the bind, which is the local disk.
        let up = "\
22 1 0:21 / / rw - overlay overlay rw
40 22 8:1 /srv/redeluge/archive /mnt/archive rw - ext4 /dev/sda1 rw
57 40 0:52 / /mnt/archive/s4 rw - fuse.rclone s4:torrent rw
";
        let down = "\
22 1 0:21 / / rw - overlay overlay rw
40 22 8:1 /srv/redeluge/archive /mnt/archive rw - ext4 /dev/sda1 rw
";
        assert_eq!(
            covering_fstype(up, "/mnt/archive/s4").as_deref(),
            Some("fuse.rclone")
        );
        assert_eq!(
            covering_fstype(up, "/mnt/archive/s4/films").as_deref(),
            Some("fuse.rclone")
        );
        assert_eq!(
            covering_fstype(down, "/mnt/archive/s4").as_deref(),
            Some("ext4")
        );
        assert_eq!(
            covering_fstype(up, "/mnt/archive/s40").as_deref(),
            Some("ext4")
        );
        assert_eq!(
            covering_fstype(down, "/downloads").as_deref(),
            Some("overlay")
        );

        assert!(is_remote("fuse.rclone"));
        assert!(is_remote("nfs4"));
        assert!(is_remote("cifs"));
        assert!(!is_remote("ext4"));
        assert!(!is_remote("overlay"));
        assert!(!is_remote("xfs"));

        assert!(!probe_blocking("/definitely/not/here"));
    }
}
