// SPDX-License-Identifier: GPL-3.0-or-later
//! Torrents that are not to be downloaded.
//!
//! Two ways onto the list: somebody bans a torrent, or it announces to a
//! tracker that has been blocked in its tracker settings. Either way the
//! torrent is held — paused, in error with a message saying why — only for as
//! long as its *arr takes to be told, then removed: with its files if it had
//! not finished, which is partial files nobody will finish. Added again, it is
//! refused outright, and the client that sent it is told so.
//!
//! The list outlives the torrents on it. It is the record of what was refused
//! and when, it is how a banned torrent added again is recognised, and it
//! remembers what the *arr instance was told.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::manager::SessionState;

/// How many times an automatic report is tried before giving up. An *arr
/// notices a download it grabbed within a minute or two; ten passes a minute
/// apart is plenty, and Send is always there to try again.
pub const ATTEMPTS: u32 = 10;

/// What the *arr instance has been told about one entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrState {
    /// Nothing to tell, or nobody set up to tell it to.
    #[default]
    Off,
    /// To be told at the next pass.
    Pending,
    Blocklisted,
    /// The instance had no download by this hash.
    NotInQueue,
    Failed,
}

impl ArrState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Pending => "pending",
            Self::Blocklisted => "blocklisted",
            Self::NotInQueue => "not_in_queue",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Arr {
    #[serde(default)]
    pub state: ArrState,
    /// When it was last tried, Unix seconds.
    #[serde(default)]
    pub at: f64,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub message: String,
}

/// One banned torrent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// When it was banned, Unix seconds.
    pub at: f64,
    /// Its name, once known: a magnet has none until its metadata arrives,
    /// and the entry is filled in when it does.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub tracker: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub arr: Arr,
}

/// `banned.json` beside `core.conf`, keyed by info hash.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Banned {
    #[serde(default)]
    pub torrents: BTreeMap<String, Entry>,
}

/// What one look at the session found.
#[derive(Default)]
pub struct Found {
    /// The entries to tell an *arr about now: hash, label.
    pub to_send: Vec<(String, String)>,
    /// Torrents banned in this pass, because of their tracker: hash, name,
    /// reason. Re-holding one that was already banned, as every pass after a
    /// restart does, is not news and is not reported.
    pub held: Vec<(String, String, String)>,
}

impl Banned {
    fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("banned.json")
    }

    pub fn load(config_dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(config_dir))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        let path = Self::path(config_dir);
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec(self)?)?;
        std::fs::rename(&temporary, &path)
    }

    pub fn contains(&self, hash: &str) -> bool {
        self.torrents.contains_key(hash)
    }
}

/// The message a held torrent shows, and how the daemon recognises one.
pub fn held_message(reason: &str) -> String {
    format!("Blocked: {reason}")
}

/// Bans a torrent, or answers false if it already was. Filled in from the
/// session when the torrent is there.
pub fn ban(state: &mut SessionState, hash: &str, reason: &str, sends: bool, now: f64) -> bool {
    if state.banned.contains(hash) {
        return false;
    }
    let mut entry = Entry {
        at: now,
        name: hash.to_owned(),
        reason: reason.to_owned(),
        ..Entry::default()
    };
    if let Ok(status) = state.session.torrent_status(hash) {
        let trackers = state.tracker_list(&status, false);
        let announced = crate::torrent::current_tracker(&status.current_tracker, &trackers);
        entry.tracker = crate::torrent::tracker_host(&announced);
        if !status.name.is_empty() {
            entry.name = status.name;
        }
    }
    if let Some(torrent) = state.torrents.get(hash) {
        entry.label = torrent.options.label.clone();
    }
    if sends {
        entry.arr.state = ArrState::Pending;
    }
    state.banned.torrents.insert(hash.to_owned(), entry);
    true
}

/// Holds every torrent that is banned or announces to a blocked tracker,
/// bans the latter, fills in names that have become known, and picks out
/// the entries an *arr is due to hear about.
///
/// Saves the list when anything changed. `sends` says whether a label's
/// torrents are reported without being asked.
pub fn enforce(
    state: &mut SessionState,
    blocked_trackers: &BTreeSet<String>,
    sends: &dyn Fn(&str) -> bool,
    now: f64,
) -> Found {
    let mut found = Found::default();
    let mut changed = false;

    for status in state.session.all_torrent_status() {
        let hash = status.info_hash.clone();
        if !state.torrents.contains_key(&hash) {
            continue;
        }
        let new = !state.banned.contains(&hash);
        if new {
            let trackers = state.tracker_list(&status, false);
            let announced = crate::torrent::current_tracker(&status.current_tracker, &trackers);
            let host = crate::torrent::tracker_host(&announced);
            if !blocked_trackers.contains(&host) {
                continue;
            }
            let label = state.torrents[&hash].options.label.clone();
            ban(
                state,
                &hash,
                &format!("its tracker {host} is blocked"),
                sends(&label),
                now,
            );
            changed = true;
        }

        let entry = state.banned.torrents.get_mut(&hash).expect("just checked");
        // A magnet's name arrives with its metadata.
        if (entry.name.is_empty() || entry.name == hash)
            && status.has_metadata
            && !status.name.is_empty()
        {
            entry.name = status.name.clone();
            changed = true;
        }
        let message = held_message(&entry.reason);
        let (name, reason) = (entry.name.clone(), entry.reason.clone());

        // Held again if anything started it: the ban is the statement that
        // stands until somebody lifts it. Out of the queue's hands too, or
        // the queue would start it again.
        let torrent = state.torrents.get_mut(&hash).expect("just checked");
        if torrent.forced_error.as_deref() != Some(message.as_str()) {
            torrent.forced_error = Some(message);
        }
        if new {
            found.held.push((hash.clone(), name, reason));
        }
        if !status.is_paused || status.is_auto_managed() {
            hold(state, &hash);
        }
    }

    for (hash, entry) in &mut state.banned.torrents {
        if entry.arr.state != ArrState::Pending {
            continue;
        }
        if entry.arr.attempts >= ATTEMPTS {
            entry.arr.state = ArrState::NotInQueue;
            changed = true;
            continue;
        }
        if entry.arr.attempts == 0 || now - entry.arr.at >= 60.0 {
            found.to_send.push((hash.clone(), entry.label.clone()));
        }
    }

    if changed {
        if let Err(err) = state.banned.save(&state.config_dir) {
            tracing::warn!(error = %err, "could not save the banned torrents");
        }
    }
    found
}

/// The banned torrents still in the session whose *arr has had its answer (or
/// has none to give), so nothing is left to keep them for: hash, name, and
/// whether their files go with them. An unfinished download is partial files
/// nobody will finish; a finished one keeps its files.
pub fn due_to_drop(state: &SessionState) -> Vec<(String, String, bool)> {
    use redeluge_libtorrent::TorrentState as Lt;
    state
        .banned
        .torrents
        .iter()
        .filter(|(hash, entry)| {
            entry.arr.state != ArrState::Pending && state.torrents.contains_key(*hash)
        })
        .filter_map(|(hash, entry)| {
            let status = state.session.torrent_status(hash).ok()?;
            // Not while its files are being checked: until the check is done a
            // complete download does not look finished, and would lose files
            // it has every right to keep. The next pass looks again.
            if matches!(
                status.state,
                Lt::CheckingFiles | Lt::CheckingResumeData | Lt::Other(_)
            ) {
                return None;
            }
            Some((hash.clone(), entry.name.clone(), !status.is_finished))
        })
        .collect()
}

/// A banned torrent sent again and taken in to be reported: held from the
/// first moment, and its entry set to tell the *arr afresh, which then puts
/// this attempt on its blocklist too. The sweep removes it once that is done.
pub fn returned(state: &mut SessionState, hash: &str, reason: &str) {
    if let Some(torrent) = state.torrents.get_mut(hash) {
        torrent.forced_error = Some(held_message(reason));
    }
    hold(state, hash);
    if let Some(entry) = state.banned.torrents.get_mut(hash) {
        entry.arr = Arr {
            state: ArrState::Pending,
            ..Arr::default()
        };
    }
    if let Err(err) = state.banned.save(&state.config_dir) {
        tracing::warn!(error = %err, "could not save the banned torrents");
    }
}

/// Pauses a torrent and takes it out of the queue's hands, as pausing it by
/// hand does.
pub fn hold(state: &mut SessionState, hash: &str) {
    let change = redeluge_libtorrent::FlagChange::new()
        .set_to(redeluge_libtorrent::flags::PAUSED, true)
        .set_to(redeluge_libtorrent::flags::AUTO_MANAGED, false);
    let _ = state.session.set_flags(hash, change);
    if let Some(torrent) = state.torrents.get_mut(hash) {
        torrent.options.paused = true;
        torrent.options.auto_managed = false;
    }
    state.mark_dirty();
}

/// Records what came of telling an *arr, and saves.
pub fn record_arr(
    state: &mut SessionState,
    hash: &str,
    outcome: &Result<crate::arr::Outcome, String>,
    automatic: bool,
    now: f64,
) {
    let Some(entry) = state.banned.torrents.get_mut(hash) else {
        return;
    };
    entry.arr.at = now;
    entry.arr.attempts += 1;
    (entry.arr.state, entry.arr.message) = match outcome {
        Ok(crate::arr::Outcome::Blocklisted) => (ArrState::Blocklisted, String::new()),
        // Not there yet is worth another look on the automatic path: the
        // instance may not have noticed what it grabbed.
        Ok(crate::arr::Outcome::NotInQueue) if automatic && entry.arr.attempts < ATTEMPTS => {
            (ArrState::Pending, "not in its queue yet".into())
        }
        Ok(crate::arr::Outcome::NotInQueue) => (ArrState::NotInQueue, "not in its queue".into()),
        // A blip, such as the instance restarting, is worth another try too.
        Err(err) if automatic && entry.arr.attempts < ATTEMPTS => (ArrState::Pending, err.clone()),
        Err(err) => (ArrState::Failed, err.clone()),
    };
    if let Err(err) = state.banned.save(&state.config_dir) {
        tracing::warn!(error = %err, "could not save the banned torrents");
    }
}

/// Lifts bans, and the hold on any of those torrents still in the session.
/// They stay paused: what happens next is somebody's choice.
pub fn unban(state: &mut SessionState, hashes: &[String]) -> usize {
    let mut lifted = 0;
    for hash in hashes {
        let Some(entry) = state.banned.torrents.remove(hash) else {
            continue;
        };
        lifted += 1;
        if let Some(torrent) = state.torrents.get_mut(hash) {
            if torrent.forced_error.as_deref() == Some(held_message(&entry.reason).as_str()) {
                torrent.forced_error = None;
            }
        }
    }
    if lifted > 0 {
        if let Err(err) = state.banned.save(&state.config_dir) {
            tracing::warn!(error = %err, "could not save the banned torrents");
        }
    }
    lifted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut banned = Banned::default();
        banned.torrents.insert(
            "abc".into(),
            Entry {
                at: 1.0,
                name: "Some.Release".into(),
                arr: Arr {
                    state: ArrState::Pending,
                    ..Arr::default()
                },
                ..Entry::default()
            },
        );
        banned.save(dir.path()).unwrap();
        let again = Banned::load(dir.path());
        assert_eq!(again.torrents, banned.torrents);
        assert!(std::fs::read_to_string(dir.path().join("banned.json"))
            .unwrap()
            .contains("\"pending\""));
    }
}
