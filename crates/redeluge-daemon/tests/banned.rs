// SPDX-License-Identifier: GPL-3.0-or-later
//! A banned torrent goes, and does not come back.
//!
//! 1.9.0 held a banned torrent in error and left it there, and adding it again
//! only held it again. What a ban has to mean is that the torrent is removed
//! and that the client refuses it from then on, whatever sent it.

use std::sync::Arc;

use redeluge_daemon::auth::{AuthLevel, AuthManager};
use redeluge_daemon::config::Config;
use redeluge_daemon::core::Core;
use redeluge_daemon::events::Event;
use redeluge_daemon::manager::Manager;
use redeluge_daemon::rpc::{CallContext, Rpc};
use redeluge_libtorrent::SessionSettings;
use redeluge_rencode::Value;

const MAGNET: &str = "magnet:?xt=urn:btih:4444444444444444444444444444444444444444&dn=Some.Release";
const HASH: &str = "4444444444444444444444444444444444444444";

async fn daemon() -> (Arc<Core>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let config = Config::load(dir.path()).expect("a fresh config");
    let auth = AuthManager::open(dir.path()).expect("an auth file");
    let (events, _) = tokio::sync::broadcast::channel::<Event>(64);
    let manager = Manager::start(dir.path().to_path_buf(), SessionSettings::offline(), events)
        .expect("the manager starts");
    (
        Core::new(manager, config, auth, dir.path().to_path_buf()),
        dir,
    )
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

async fn present(core: &Core) -> bool {
    let all = call(
        core,
        "core.get_torrents_status",
        vec![Value::Dict(Vec::new()), Value::List(Vec::new())],
    )
    .await
    .expect("the status");
    all.get(HASH).is_some()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_banned_torrent_is_removed_and_refused_when_added_again() {
    let (core, _dir) = daemon().await;
    let add = || {
        call(
            &core,
            "core.add_torrent_magnet",
            vec![Value::Str(MAGNET.into()), Value::Dict(Vec::new())],
        )
    };

    assert_eq!(add().await, Ok(Value::Str(HASH.into())));
    assert!(present(&core).await);

    let answers = call(
        &core,
        "redeluge.ban_torrents",
        vec![
            Value::List(vec![Value::Str(HASH.into())]),
            Value::Str("bad release".into()),
            Value::Bool(false),
        ],
    )
    .await
    .expect("the ban");
    assert_eq!(
        answers.as_list().and_then(|list| list[0].get("removed")),
        Some(&Value::Bool(true)),
        "{answers:?}"
    );
    assert!(
        !present(&core).await,
        "a banned torrent stayed in the session"
    );

    let refused = add().await.expect_err("a banned torrent was added again");
    assert!(
        refused.starts_with("AddTorrentError: this torrent is banned: bad release"),
        "{refused}"
    );
    assert!(
        !present(&core).await,
        "the refused add left the torrent behind"
    );

    // Lifted, it can come back.
    call(
        &core,
        "redeluge.unban_torrents",
        vec![Value::List(vec![Value::Str(HASH.into())])],
    )
    .await
    .expect("the unban");
    assert_eq!(add().await, Ok(Value::Str(HASH.into())));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_banned_torrent_sent_again_is_held_when_its_arr_asks_for_returns() {
    let (core, _dir) = daemon().await;
    let add = || {
        call(
            &core,
            "core.add_torrent_magnet",
            vec![Value::Str(MAGNET.into()), Value::Dict(Vec::new())],
        )
    };
    let text = |value: &str| Value::Str(value.into());

    assert_eq!(add().await, Ok(text(HASH)));
    call(&core, "label.add", vec![text("tv")])
        .await
        .expect("a label");
    call(&core, "label.set_torrent", vec![text(HASH), text("tv")])
        .await
        .expect("labelled");
    // Nothing answers there; the sweep that would tell it does not run here.
    call(
        &core,
        "redeluge.set_arr",
        vec![
            text("tv"),
            Value::Dict(vec![
                (text("url"), text("http://127.0.0.1:9")),
                (text("api_key"), text("key")),
                (text("report_returns"), Value::Bool(true)),
            ]),
        ],
    )
    .await
    .expect("the *arr");

    call(
        &core,
        "redeluge.ban_torrents",
        vec![
            Value::List(vec![text(HASH)]),
            text("bad release"),
            Value::Bool(false),
        ],
    )
    .await
    .expect("the ban");
    assert!(!present(&core).await);

    // Taken in rather than refused, and held: paused, saying why.
    assert_eq!(add().await, Ok(text(HASH)));
    let status = call(
        &core,
        "core.get_torrent_status",
        vec![
            text(HASH),
            Value::List(vec![text("paused"), text("message")]),
        ],
    )
    .await
    .expect("its status");
    assert_eq!(status.get("paused"), Some(&Value::Bool(true)), "{status:?}");
    assert_eq!(
        status.get("message"),
        Some(&text("Blocked: bad release")),
        "{status:?}"
    );

    // And due to be reported afresh.
    let banned = call(&core, "redeluge.get_banned", Vec::new())
        .await
        .expect("the list");
    let entry = &banned.as_list().expect("a list")[0];
    assert_eq!(entry.get("arr_state"), Some(&text("pending")), "{entry:?}");
}
