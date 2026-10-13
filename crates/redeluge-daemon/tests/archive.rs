// SPDX-License-Identifier: GPL-3.0-or-later
//! Archive targets that are down when the daemon starts.
//!
//! The failure this guards against is the expensive one: a restart while the
//! mount is gone finds no files, checks the torrent to nothing and, once
//! resumed, downloads it all again into the empty mount point. A target that
//! is not a FUSE or network mount — a plain directory, here — is offline, which
//! is exactly what a stopped rclone leaves behind.

use std::sync::Arc;

use redeluge_daemon::auth::{AuthLevel, AuthManager};
use redeluge_daemon::config::Config;
use redeluge_daemon::core::Core;
use redeluge_daemon::events::Event;
use redeluge_daemon::manager::Manager;
use redeluge_daemon::rpc::{CallContext, Rpc};
use redeluge_libtorrent::SessionSettings;
use redeluge_rencode::Value;
use serde_json::json;

const HASH: &str = "5555555555555555555555555555555555555555";
const MAGNET: &str = "magnet:?xt=urn:btih:5555555555555555555555555555555555555555&dn=Archived";

/// A daemon whose previous run left one torrent at `save_path`, with an
/// archive target at `<dir>/archive`.
async fn restarted_with_torrent_at(save_path: &str) -> (Arc<Core>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let target = dir.path().join("archive");
    std::fs::create_dir_all(&target).expect("the mount point");
    let save_path = save_path.replace("<dir>", &dir.path().to_string_lossy());

    let state = dir.path().join("state");
    std::fs::create_dir_all(&state).expect("the state directory");
    std::fs::write(
        state.join("torrents.json"),
        serde_json::to_vec(&json!({
            "version": 1,
            "torrents": [{"torrent_id": HASH, "magnet": MAGNET, "save_path": save_path}],
        }))
        .unwrap(),
    )
    .expect("the state file");

    let mut config = Config::load(dir.path()).expect("a fresh config");
    config
        .set(
            "archive",
            json!({"targets": [{"name": "s4", "path": target.to_string_lossy()}]}),
        )
        .expect("the archive setting");
    let auth = AuthManager::open(dir.path()).expect("an auth file");
    let (events, _) = tokio::sync::broadcast::channel::<Event>(64);
    let manager = Manager::start(dir.path().to_path_buf(), SessionSettings::offline(), events)
        .expect("the manager starts");
    let core = Core::new(manager, config, auth, dir.path().to_path_buf());
    assert_eq!(core.restore().await, 1, "the torrent is restored");
    (core, dir)
}

fn admin() -> CallContext {
    CallContext {
        session_id: 1,
        username: "localclient".to_owned(),
        level: AuthLevel::Admin,
        peer: "127.0.0.1:0".to_owned(),
    }
}

async fn call(core: &Core, method: &str, args: Vec<Value>) -> Result<Value, String> {
    core.call(&admin(), method, args, Vec::new())
        .await
        .map_err(|err| format!("{}: {}", err.exception, err.message))
}

async fn state_of(core: &Core) -> String {
    let all = call(
        core,
        "core.get_torrents_status",
        vec![
            Value::Dict(Vec::new()),
            Value::List(vec![Value::Str("state".into())]),
        ],
    )
    .await
    .expect("the status");
    all.get(HASH)
        .and_then(|row| row.get("state"))
        .and_then(Value::as_str)
        .expect("the torrent's state")
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_torrent_on_an_archive_that_is_down_at_startup_is_held_paused() {
    let (core, _dir) = restarted_with_torrent_at("<dir>/archive/films").await;

    assert_eq!(state_of(&core).await, "Paused");
    let (held, managed) = core
        .manager
        .with(|state| {
            let options = &state.torrents.get(HASH).expect("the torrent").options;
            (options.archive_paused, options.archive_was_managed)
        })
        .await
        .unwrap();
    assert!(
        held,
        "held by the archive rule, so it is let go when the target is back"
    );
    assert!(managed, "and given back to the queue then, as it was");
}

#[tokio::test(flavor = "multi_thread")]
async fn its_files_cannot_be_deleted_while_the_archive_is_down() {
    let (core, _dir) = restarted_with_torrent_at("<dir>/archive/films").await;

    let with_files = call(
        &core,
        "core.remove_torrent",
        vec![Value::Str(HASH.into()), Value::Bool(true)],
    )
    .await;
    assert!(
        with_files
            .as_ref()
            .is_err_and(|err| err.contains("archive that is offline")),
        "a deletion on a mount that is down has to wait: {with_files:?}"
    );

    // Keeping the files is still allowed: nothing is touched on the mount.
    let without = call(
        &core,
        "core.remove_torrent",
        vec![Value::Str(HASH.into()), Value::Bool(false)],
    )
    .await;
    assert!(without.is_ok(), "{without:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_torrent_elsewhere_is_restored_as_it_was() {
    let (core, _dir) = restarted_with_torrent_at("<dir>/downloads").await;

    assert_ne!(state_of(&core).await, "Paused");
    let held = core
        .manager
        .with(|state| state.torrents.get(HASH).unwrap().options.archive_paused)
        .await
        .unwrap();
    assert!(!held);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_storage_and_archive_reports_answer_without_a_mount() {
    let (core, _dir) = restarted_with_torrent_at("<dir>/archive/films").await;

    let storage = call(&core, "redeluge.get_storage", Vec::new())
        .await
        .expect("the storage report");
    let rows = storage.as_list().expect("a list");
    let archive = rows
        .iter()
        .find(|row| row.get("kind").and_then(Value::as_str) == Some("archive"))
        .expect("the target is listed");
    assert_eq!(archive.get("online"), Some(&Value::Bool(false)));
    assert_eq!(archive.get("torrents"), Some(&Value::Int(1)));

    let info = call(&core, "redeluge.get_archive_info", Vec::new())
        .await
        .expect("the archive report");
    let target = &info.as_list().expect("a list")[0];
    assert_eq!(target.get("name").and_then(Value::as_str), Some("s4"));
    assert_eq!(target.get("rc").and_then(Value::as_str), Some("none"));
}
