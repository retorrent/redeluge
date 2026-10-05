// SPDX-License-Identifier: GPL-3.0-or-later
//! What sits in a directory that no torrent accounts for.
//!
//! For purging a disk after something went wrong: a torrent removed without
//! its data, a move that stopped halfway, a client that was replaced. One
//! directory at a time and not recursive, because the entries a person picks
//! from are the top-level ones a torrent would have written there.
//!
//! Deleting is the dangerous half, so it re-decides what is claimed at the
//! moment it deletes rather than trusting the list the person was shown: a
//! torrent added in between must not lose its files to a stale listing.

use std::collections::HashSet;
use std::path::{Component, Path};

use crate::manager::SessionState;

/// The names directly inside `dir` that some torrent's files live under,
/// in lower case: on a disk that ignores case, `Show` on disk is the torrent's
/// `show`, and hiding a real orphan that differs only in case is the safe way
/// to be wrong.
///
/// A torrent claims an entry when any of its files would be written below it,
/// from where it is now or from where it is being moved to, and when its save
/// path itself is below `dir`, whether or not its files are known yet. A
/// magnet still waiting for metadata has no files, so its name stands in for
/// them, and libtorrent's part file is claimed beside it.
pub fn claimed_in(state: &SessionState, dir: &Path) -> HashSet<String> {
    // Both spellings of every path, as written and with its links resolved:
    // a library reached through a link must not look like nobody's files.
    let spellings = |path: &Path| {
        let mut both = vec![path.to_path_buf()];
        both.extend(path.canonicalize().ok().filter(|real| real != path));
        both
    };
    let dirs = spellings(dir);

    let mut claimed = HashSet::new();
    let mut claim = |path: &Path| {
        for dir in &dirs {
            if let Ok(rest) = path.strip_prefix(dir) {
                if let Some(Component::Normal(first)) = rest.components().next() {
                    claimed.insert(first.to_string_lossy().to_lowercase());
                }
            }
        }
    };

    for status in state.session.all_torrent_status() {
        let moving_to = state
            .torrents
            .get(&status.info_hash)
            .and_then(|torrent| torrent.moving_to.clone());

        for written in std::iter::once(status.save_path.clone()).chain(moving_to) {
            for root in spellings(Path::new(&written)) {
                // Neither inside the other: nothing of this torrent can be
                // here, and its file list is not worth asking for.
                if !dirs
                    .iter()
                    .any(|dir| root.starts_with(dir) || dir.starts_with(&root))
                {
                    continue;
                }
                claim(&root);
                claim(&root.join(&status.name));
                claim(&root.join(format!(".{}.parts", status.info_hash)));
                for file in state.session.files(&status.info_hash).unwrap_or_default() {
                    claim(&root.join(&file.path));
                }
            }
        }
    }
    claimed
}

/// The directory a cleanup was asked about, if it is one it can be trusted
/// with: absolute, and with no `..` that would make a torrent's path look
/// like it is somewhere else.
pub fn directory(path: &str) -> Result<&Path, String> {
    let dir = Path::new(path);
    if !dir.is_absolute() || dir.components().any(|part| part == Component::ParentDir) {
        return Err("an absolute path without \"..\" is required".into());
    }
    if !dir.is_dir() {
        return Err(format!("{path} is not a directory"));
    }
    Ok(dir)
}

/// One entry nobody claims.
pub struct Orphan {
    pub name: String,
    pub directory: bool,
    /// Everything under it, for a directory.
    pub size: i64,
}

/// What is directly inside `dir` and not in `claimed`, sorted by name.
pub fn orphans(dir: &Path, claimed: &HashSet<String>) -> std::io::Result<Vec<Orphan>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if claimed.contains(&name.to_lowercase()) {
            continue;
        }
        // Not following links: a link is what would be deleted, not what it
        // points at.
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        let directory = meta.is_dir();
        let size = if directory {
            crate::core::path_size(&entry.path().to_string_lossy())
        } else {
            meta.len() as i64
        };
        found.push(Orphan {
            name,
            directory,
            size,
        });
    }
    found.sort_by_key(|orphan| orphan.name.to_lowercase());
    Ok(found)
}

/// Deletes `name` from `dir`, if it is a plain entry of it nobody claims.
pub fn delete(dir: &Path, name: &str, claimed: &HashSet<String>) -> Result<(), String> {
    // One component, and an ordinary one: no way out of `dir` from here.
    let mut parts = Path::new(name).components();
    if !matches!(
        (parts.next(), parts.next()),
        (Some(Component::Normal(_)), None)
    ) {
        return Err("not an entry of this directory".into());
    }
    if claimed.contains(&name.to_lowercase()) {
        return Err("a torrent's files are in it".into());
    }
    let path = dir.join(name);
    let meta = path.symlink_metadata().map_err(|err| err.to_string())?;
    if meta.is_dir() {
        std::fs::remove_dir_all(&path)
    } else {
        std::fs::remove_file(&path)
    }
    .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_nobody_claims_is_listed_or_deleted() {
        let dir = std::env::temp_dir().join(format!("redeluge-cleanup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("kept/inner")).unwrap();
        std::fs::create_dir_all(dir.join("stale/inner")).unwrap();
        std::fs::write(dir.join("stale/inner/file"), b"12345").unwrap();
        std::fs::write(dir.join("loose.bin"), b"123").unwrap();

        let claimed: HashSet<String> = ["kept".to_owned()].into();
        let found = orphans(&dir, &claimed).unwrap();
        let names: Vec<&str> = found.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["loose.bin", "stale"]);
        assert_eq!(found[1].size, 5);
        assert!(found[1].directory);

        // Nothing outside the directory, and nothing claimed.
        assert!(delete(&dir, "../x", &claimed).is_err());
        assert!(delete(&dir, "stale/inner", &claimed).is_err());
        assert!(delete(&dir, "..", &claimed).is_err());
        assert!(delete(&dir, "", &claimed).is_err());
        assert!(delete(&dir, "kept", &claimed).is_err());
        // A torrent's folder in other capitals is still the torrent's.
        assert!(delete(&dir, "KEPT", &claimed).is_err());
        assert!(dir.join("kept").exists());

        delete(&dir, "stale", &claimed).unwrap();
        delete(&dir, "loose.bin", &claimed).unwrap();
        assert!(orphans(&dir, &claimed).unwrap().is_empty());

        assert!(directory("relative").is_err());
        assert!(directory(&format!("{}/kept/..", dir.display())).is_err());
        assert!(directory(&dir.display().to_string()).is_ok());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
