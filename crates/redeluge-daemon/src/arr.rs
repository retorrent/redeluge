// SPDX-License-Identifier: GPL-3.0-or-later
//! Telling *arr software that a release is not wanted.
//!
//! The *arr programs keep their own blocklist of releases. Removing a download
//! from their queue with `blocklist=true` puts the release on it, and unless
//! told otherwise they go and look for another one. That is the only way a
//! download client can say "not this one": the *arr side of Deluge's API never
//! reports a torrent as failed, only as a warning nothing acts on.
//!
//! Which instance to tell is decided by the torrent's label, because the label
//! is what an *arr sets as its category: one label per instance, and the label
//! names who to call. The settings live in their own file rather than under
//! the `label` key of `core.conf`, because an API key is a secret and every
//! read of the configuration, by any account, would otherwise hand it out.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// How long one request may take. The automatic reports run inside the
/// tracker rules' sweep, and an instance that accepts a connection and never
/// answers must not hold up every other rule.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// One label's instance.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Target {
    /// Where the instance answers, such as `http://host:port`. A base path
    /// set in the instance goes here too.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub api_key: String,
    /// Tell it, without being asked, about every torrent of this label that
    /// is blocked. Off: a blocked torrent waits for somebody to press Send.
    #[serde(default)]
    pub send_blocklist: bool,
}

impl Target {
    /// Whether there is anything to call.
    pub fn usable(&self) -> bool {
        !self.url.trim().is_empty() && !self.api_key.trim().is_empty()
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/api/v3/{path}", self.url.trim().trim_end_matches('/'))
    }
}

/// Every label's instance, in `arr.json` beside `core.conf`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub labels: BTreeMap<String, Target>,
}

impl Settings {
    fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("arr.json")
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
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        // Owner only: it holds API keys.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&temporary, &path)
    }

    pub fn target(&self, label: &str) -> Option<&Target> {
        self.labels.get(label).filter(|target| target.usable())
    }

    /// Whether the torrents of this label are reported without being asked.
    pub fn sends(&self, label: &str) -> bool {
        self.target(label)
            .is_some_and(|target| target.send_blocklist)
    }
}

/// What came of telling an instance.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// It is on the instance's blocklist now.
    Blocklisted,
    /// The instance has no download by that hash: it did not grab this one,
    /// or has not noticed it yet.
    NotInQueue,
}

/// Puts the download with this info hash on the instance's blocklist and
/// takes it out of its queue, leaving the torrent itself alone.
///
/// `removeFromClient=false` because the daemon decides what happens to its
/// own torrents; `skipRedownload=false` so the instance looks for another
/// release, which is the point. Every *arr on API v3 speaks this.
pub async fn blocklist(
    client: &reqwest::Client,
    target: &Target,
    hash: &str,
) -> Result<Outcome, String> {
    let queue: Json = client
        .get(target.endpoint("queue/details"))
        .query(&[
            ("includeUnknownSeriesItems", "true"),
            ("includeUnknownMovieItems", "true"),
        ])
        .header("X-Api-Key", target.api_key.trim())
        .timeout(TIMEOUT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(describe)?
        .bytes()
        .await
        .map_err(describe)
        .and_then(|body| {
            serde_json::from_slice(&body).map_err(|err| format!("not an *arr queue: {err}"))
        })?;

    // Deluge's download id, as the *arr records it, is the info hash in
    // capitals. Compared without case anyway.
    let Some(id) = queue.as_array().and_then(|items| {
        items.iter().find_map(|item| {
            let theirs = item.get("downloadId")?.as_str()?;
            theirs
                .eq_ignore_ascii_case(hash)
                .then(|| item.get("id")?.as_i64())
                .flatten()
        })
    }) else {
        return Ok(Outcome::NotInQueue);
    };

    client
        .delete(target.endpoint(&format!("queue/{id}")))
        .query(&[
            ("removeFromClient", "false"),
            ("blocklist", "true"),
            ("skipRedownload", "false"),
        ])
        .header("X-Api-Key", target.api_key.trim())
        .timeout(TIMEOUT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(describe)?;
    Ok(Outcome::Blocklisted)
}

/// Asks the instance who it is (its name and version), for a settings window's Test.
pub async fn identify(client: &reqwest::Client, target: &Target) -> Result<String, String> {
    let status: Json = client
        .get(target.endpoint("system/status"))
        .header("X-Api-Key", target.api_key.trim())
        .timeout(TIMEOUT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(describe)?
        .bytes()
        .await
        .map_err(describe)
        .and_then(|body| {
            serde_json::from_slice(&body).map_err(|err| format!("not an *arr: {err}"))
        })?;
    let name = status.get("appName").and_then(Json::as_str).unwrap_or("?");
    let version = status.get("version").and_then(Json::as_str).unwrap_or("?");
    Ok(format!("{name} {version}"))
}

/// An HTTP failure in words somebody can act on. A 401 is the API key.
fn describe(err: reqwest::Error) -> String {
    match err.status() {
        Some(reqwest::StatusCode::UNAUTHORIZED) => "the API key was refused".into(),
        Some(status) => format!("the instance answered {status}"),
        None => format!("could not reach it: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// An *arr that answers one queue and records what it was asked.
    async fn fake_arr(queue: &'static str) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            for _ in 0..2 {
                let Ok(Ok((mut socket, _))) =
                    tokio::time::timeout(std::time::Duration::from_secs(2), listener.accept())
                        .await
                else {
                    break;
                };
                let mut buffer = vec![0u8; 8192];
                let read = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let line = request.lines().next().unwrap_or_default().to_owned();
                let authorised = request.to_lowercase().contains("x-api-key: secret");
                seen.push(format!("{line} key={authorised}"));
                let body = if line.starts_with("GET") { queue } else { "" };
                let status = if authorised {
                    "200 OK"
                } else {
                    "401 Unauthorized"
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
            seen
        });
        (url, handle)
    }

    fn target(url: &str, key: &str) -> Target {
        Target {
            url: format!("{url}/"),
            api_key: key.into(),
            send_blocklist: true,
        }
    }

    #[tokio::test]
    async fn the_matching_download_is_blocklisted_and_left_in_the_client() {
        let (url, server) =
            fake_arr(r#"[{"id":7,"downloadId":"OTHER"},{"id":42,"downloadId":"ABCDEF0123"}]"#)
                .await;
        let outcome = blocklist(
            &reqwest::Client::new(),
            &target(&url, "secret"),
            "abcdef0123",
        )
        .await
        .unwrap();
        assert_eq!(outcome, Outcome::Blocklisted);

        let seen = server.await.unwrap();
        assert!(
            seen[0].starts_with("GET /api/v3/queue/details?"),
            "{seen:?}"
        );
        assert_eq!(
            seen[1],
            "DELETE /api/v3/queue/42?removeFromClient=false&blocklist=true&skipRedownload=false HTTP/1.1 key=true"
        );
    }

    #[tokio::test]
    async fn a_download_the_instance_does_not_have_is_said_so() {
        let (url, server) = fake_arr(r#"[{"id":7,"downloadId":"OTHER"}]"#).await;
        let outcome = blocklist(
            &reqwest::Client::new(),
            &target(&url, "secret"),
            "abcdef0123",
        )
        .await
        .unwrap();
        assert_eq!(outcome, Outcome::NotInQueue);
        // Nothing deleted.
        assert_eq!(server.await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_wrong_key_is_named() {
        let (url, _server) = fake_arr("[]").await;
        let err = blocklist(&reqwest::Client::new(), &target(&url, "wrong"), "abc")
            .await
            .unwrap_err();
        assert_eq!(err, "the API key was refused");
    }
}
