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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub targets: Vec<Target>,
    /// How many torrents may be copied into or out of archives at once, all
    /// targets together. One unless somebody raises it: each copy is the
    /// whole torrent through the local disk.
    #[serde(default = "one")]
    pub max_moves: u32,
    /// How many torrents may be on their way into each target at once —
    /// being copied in, or uploaded by rclone — or zero for no limit.
    #[serde(default)]
    pub max_uploads: u32,
}

fn one() -> u32 {
    1
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            max_moves: one(),
            max_uploads: 0,
        }
    }
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

    /// Copies at once, bounded: zero would stop archiving altogether, and a
    /// figure from a client in the thousands would only fill the disk.
    pub fn moves_allowed(&self) -> usize {
        self.max_moves.clamp(1, 16) as usize
    }

    /// Torrents on their way into one target at once, when limited.
    pub fn uploads_allowed(&self) -> Option<usize> {
        (self.max_uploads > 0).then(|| self.max_uploads.min(64) as usize)
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
    // FUSE is `fuse.<program>`; plain `fuse` and `fuseblk` are local disks
    // (ntfs-3g and the like). The network ones are matched by prefix:
    // `nfs4`, `smb3` are spelled that way.
    if let Some(program) = fstype.strip_prefix("fuse.") {
        return !program.is_empty();
    }
    REMOTE_FILESYSTEMS
        .iter()
        .filter(|remote| **remote != "fuse")
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

/// The HTTP client every call to rclone's rc goes through: one connection pool
/// for the daemon rather than a new one per report.
pub fn client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
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

/// A duration for a person: `3d 4h`, `2h 10m`, `45m`, `30s`.
pub fn duration(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    match (days, hours, minutes) {
        (0, 0, 0) => format!("{seconds}s"),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

/// What the move rule knows about one torrent its label wants archived.
pub struct Candidate<'a> {
    pub target: &'a str,
    pub online: bool,
    /// Finished, with its metadata.
    pub finished: bool,
    pub errored: bool,
    pub banned: bool,
    /// Its files are on an archive that is offline right now.
    pub source_offline: bool,
    /// Seconds without a byte either way, when known.
    pub idle: Option<f64>,
    /// Seconds the label asks for.
    pub wanted: f64,
    /// It is being moved already.
    pub moving: bool,
    /// Another move is running, or one was chosen earlier in this pass.
    pub queue_busy: bool,
    pub moving_name: Option<&'a str>,
    /// The target already has as many torrents on their way in as it is
    /// allowed: the limit, to say so.
    pub target_full: Option<usize>,
}

/// What to do with a torrent a label wants archived.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Move,
    /// Its move is under way.
    Moving,
    /// Not now, and why, in words for the interface.
    Wait(String),
}

/// Whether a torrent goes to its archive now, and if not, why.
///
/// The order is the order a person would look in: things only they can fix
/// first, the passage of time after, the queue last.
pub fn decide(candidate: &Candidate) -> Verdict {
    let wait = |text: String| Verdict::Wait(text);
    if !candidate.online {
        return wait(format!("waiting: {} is offline", candidate.target));
    }
    if !candidate.finished {
        return wait("waiting: the download is not finished".to_owned());
    }
    if candidate.errored {
        return wait("waiting: the torrent is in error".to_owned());
    }
    if candidate.banned {
        return wait("not archived: the torrent is banned".to_owned());
    }
    if candidate.source_offline {
        return wait("waiting: its current location is offline".to_owned());
    }
    if candidate.idle.is_none_or(|idle| idle < candidate.wanted) {
        return wait(format!(
            "waiting: idle {} of the {} the label asks for",
            duration(candidate.idle.unwrap_or(0.0)),
            duration(candidate.wanted)
        ));
    }
    if candidate.moving {
        return Verdict::Moving;
    }
    if let Some(limit) = candidate.target_full {
        return wait(format!(
            "queued: {limit} already on their way to {}",
            candidate.target
        ));
    }
    if candidate.queue_busy {
        return wait(format!(
            "queued: copies are at their limit{}",
            candidate
                .moving_name
                .map(|name| format!(", {name} is moving"))
                .unwrap_or_default()
        ));
    }
    Verdict::Move
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

    fn ready() -> Candidate<'static> {
        Candidate {
            target: "s4",
            online: true,
            finished: true,
            errored: false,
            banned: false,
            source_offline: false,
            idle: Some(7200.0),
            wanted: 3600.0,
            moving: false,
            queue_busy: false,
            moving_name: None,
            target_full: None,
        }
    }

    #[test]
    fn a_full_target_queues_and_says_how_full() {
        let full = Candidate {
            target_full: Some(2),
            ..ready()
        };
        assert_eq!(
            decide(&full),
            Verdict::Wait("queued: 2 already on their way to s4".to_owned())
        );
    }

    #[test]
    fn the_limits_are_bounded_and_default_to_one_copy_and_no_upload_limit() {
        let stored = Settings::from_config(Some(&serde_json::json!({"targets": []})));
        assert_eq!(stored.moves_allowed(), 1);
        assert_eq!(stored.uploads_allowed(), None);

        let zero = Settings {
            max_moves: 0,
            ..stored.clone()
        };
        assert_eq!(zero.moves_allowed(), 1, "zero would stop archiving");
        let silly = Settings {
            max_moves: 900,
            max_uploads: 900,
            ..stored
        };
        assert_eq!(silly.moves_allowed(), 16);
        assert_eq!(silly.uploads_allowed(), Some(64));
    }

    #[test]
    fn a_quiet_finished_torrent_with_its_target_up_moves() {
        assert_eq!(decide(&ready()), Verdict::Move);
        // Zero hours means as soon as it finishes, even with no transfer yet.
        let at_once = Candidate {
            idle: Some(0.0),
            wanted: 0.0,
            ..ready()
        };
        assert_eq!(decide(&at_once), Verdict::Move);
    }

    #[test]
    fn every_reason_to_wait_says_so() {
        let says = |candidate: Candidate, expected: &str| match decide(&candidate) {
            Verdict::Wait(note) => assert!(note.contains(expected), "{note:?} lacks {expected:?}"),
            other => panic!("expected a wait about {expected:?}, got {other:?}"),
        };
        says(
            Candidate {
                online: false,
                ..ready()
            },
            "s4 is offline",
        );
        says(
            Candidate {
                finished: false,
                ..ready()
            },
            "not finished",
        );
        says(
            Candidate {
                errored: true,
                ..ready()
            },
            "in error",
        );
        says(
            Candidate {
                banned: true,
                ..ready()
            },
            "banned",
        );
        says(
            Candidate {
                source_offline: true,
                ..ready()
            },
            "current location is offline",
        );
        says(
            Candidate {
                idle: Some(600.0),
                ..ready()
            },
            "idle 10m of the 1h 0m",
        );
        says(
            Candidate {
                idle: None,
                ..ready()
            },
            "idle 0s of the 1h 0m",
        );
        says(
            Candidate {
                queue_busy: true,
                moving_name: Some("x.iso"),
                ..ready()
            },
            "x.iso is moving",
        );
    }

    #[test]
    fn the_first_thing_to_fix_is_the_one_shown() {
        // Offline and not finished: the target is what somebody can act on.
        let both = Candidate {
            online: false,
            finished: false,
            ..ready()
        };
        assert_eq!(
            decide(&both),
            Verdict::Wait("waiting: s4 is offline".to_owned())
        );
        // A torrent already moving is not "queued" behind itself.
        let mine = Candidate {
            moving: true,
            queue_busy: true,
            ..ready()
        };
        assert_eq!(decide(&mine), Verdict::Moving);
    }

    #[test]
    fn durations_read_like_a_person_wrote_them() {
        assert_eq!(duration(30.0), "30s");
        assert_eq!(duration(45.0 * 60.0), "45m");
        assert_eq!(duration(2.0 * 3600.0 + 600.0), "2h 10m");
        assert_eq!(duration(3.0 * 86400.0 + 4.0 * 3600.0), "3d 4h");
        assert_eq!(duration(-5.0), "0s");
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
        assert!(!is_remote("fuseblk"), "ntfs-3g on a local disk");
        assert!(!is_remote("fuse"));
        assert!(is_remote("fuse.sshfs"));
        assert!(is_remote("smb3"));

        assert!(!probe_blocking("/definitely/not/here"));
    }
}
