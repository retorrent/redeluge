# SPDX-License-Identifier: GPL-3.0-or-later
"""Archive against every other rule that moves, labels or removes torrents.

Each scenario sets the rules up, adds a fresh torrent, watches it for a while
and checks the outcome: where it ends up, how many times it was moved, and
that rclone never had an upload cancelled under it. Rules that are each right
on their own can still fight — a tracker's move rule and the archive took
turns moving the same torrent, cancelling rclone's upload every time — and
this is where that shows.

    docker compose exec lab python3 /src/docker/archive-lab/scenarios.py
    docker compose exec lab python3 /src/docker/archive-lab/scenarios.py A C

With letters, only those scenarios run.

Run on the lab only: it rewrites the tracker, label and archive settings.
"""

import base64
import subprocess
import sys
import time

sys.path.insert(0, "/src/docker/archive-lab")
import lab  # noqa: E402

TARGETS = {
    "targets": [{"name": "s3a", "path": "/mnt/a", "rc_url": "http://127.0.0.1:5572"}],
    "max_moves": 2,
    "max_uploads": 0,
}
TRACKER = "127.0.0.1"
failures = []


def rclone_deletes():
    """Cache files rclone dropped because the file was deleted under it."""
    out = subprocess.run(
        ["grep", "-c", "removed cache file as file deleted", "/var/log/rclone-a.log"],
        capture_output=True,
        text=True,
    ).stdout.strip()
    return int(out or 0)


def bucket():
    out = subprocess.run(["rclone", "lsf", "minio:archive-a"], capture_output=True, text=True)
    return set(out.stdout.split())


def reset(tracker_rule, labels):
    lab.call("core.set_config", {"archive": TARGETS})
    lab.call("core.set_config", {"tracker": {"trackers": {TRACKER: tracker_rule} if tracker_rule else {}}})
    for name, options in labels.items():
        try:
            lab.call("label.add", name)
        except SystemExit:
            pass
        base = {"apply_archive": False, "archive_target": "", "restore_local": False}
        base.update(options)
        lab.call("label.set_options", name, base)


def add(name, label=None, size=8):
    blob = lab.make_torrent(name, size * 1024 * 1024)
    torrent = lab.call(
        "core.add_torrent_file",
        f"{name}.torrent",
        base64.b64encode(blob).decode(),
        {"download_location": "/downloads"},
    )
    if label:
        lab.call("label.set_torrent", torrent, label)
    return torrent


def watch(torrent, seconds):
    """Every distinct (location, archive state, label) the torrent goes through."""
    seen = []
    end = time.time() + seconds
    while time.time() < end:
        found = lab.call("core.get_torrents_status", {}, ["save_path", "archive_state", "label", "state"])
        row = found.get(torrent)
        snap = None if row is None else (row["save_path"], row["archive_state"] or "local", row["label"], row["state"])
        if not seen or seen[-1] != snap:
            seen.append(snap)
        time.sleep(2)
    return seen


def moves_of(name):
    return [
        action
        for action in lab.call("redeluge.get_recent_actions", 200)
        if action.get("name") == name and action.get("did") == "moved"
    ]


def check(scenario, condition, what):
    print(f"   {'ok ' if condition else 'FAIL'} {what}")
    if not condition:
        failures.append(f"{scenario}: {what}")


ONLY = set(sys.argv[1:])


def wanted(letter):
    return not ONLY or letter in ONLY


def scenario(title):
    print(f"\n== {title}")
    return title


def main():
    lab.login()

    # ------------------------------------------------------------------ A
    if wanted("A"):
        s = scenario("tracker move rule + a label that archives (the ping-pong)")
        reset(
            {"auto_move": True, "move_path": "/downloads/done", "move_after_hours": 0},
            {"arch-a": {"apply_archive": True, "archive_target": "s3a", "archive_after_hours": 0}},
        )
        deletes = rclone_deletes()
        t = add("scenario-a.iso", "arch-a")
        path = watch(t, 100)
        print("   path:", " -> ".join(f"{p[0]}[{p[1]}]" for p in path if p))
        final = path[-1]
        check(s, final and final[0] == "/mnt/a" and final[1] == "archived", "ends on the archive, archived")
        check(s, len(moves_of("scenario-a.iso")) <= 2, f"moved at most twice ({len(moves_of('scenario-a.iso'))})")
        check(s, rclone_deletes() == deletes, "rclone never had the file deleted under it")
        check(s, "scenario-a.iso" in bucket(), "the file is in the bucket")

    # ------------------------------------------------------------------ B
    if wanted("B"):
        s = scenario("tracker label rule puts it in a label that archives")
        reset(
            {"auto_label": True, "label": "arch-b", "label_when_done": True, "label_after_hours": 0},
            {"arch-b": {"apply_archive": True, "archive_target": "s3a", "archive_after_hours": 0}},
        )
        deletes = rclone_deletes()
        t = add("scenario-b.iso")
        path = watch(t, 100)
        print("   path:", " -> ".join(f"{p[0]}[{p[1]},{p[2]}]" for p in path if p))
        final = path[-1]
        check(s, final and final[2] == "arch-b", "labelled by the tracker rule")
        check(s, final and final[1] == "archived", "archived")
        check(s, len(moves_of("scenario-b.iso")) == 1, f"moved once ({len(moves_of('scenario-b.iso'))})")
        check(s, rclone_deletes() == deletes, "rclone never had the file deleted under it")

    # ------------------------------------------------------------------ C
    if wanted("C"):
        s = scenario("label brings archived torrents back + tracker move rule")
        reset(
            None,
            {
                "arch-c": {"apply_archive": True, "archive_target": "s3a", "archive_after_hours": 0},
                "home-c": {"restore_local": True},
            },
        )
        t = add("scenario-c.iso", "arch-c")
        watch(t, 45)
        reset(
            {"auto_move": True, "move_path": "/downloads/done", "move_after_hours": 0},
            {"home-c": {"restore_local": True}},
        )
        lab.call("label.set_torrent", t, "home-c")
        path = watch(t, 90)
        print("   path:", " -> ".join(f"{p[0]}[{p[1]}]" for p in path if p))
        final = path[-1]
        check(s, final and final[1] == "local", "back on local disk")
        check(s, final and not final[0].startswith("/mnt/"), f"stays off the archive ({final and final[0]})")
        check(s, "scenario-c.iso" not in bucket(), "gone from the bucket")

    # ------------------------------------------------------------------ D
    if wanted("D"):
        s = scenario("tracker removal with files, on a torrent that archives")
        reset(
            {"auto_remove": True, "remove_after_hours": 0, "remove_data": True},
            {"arch-d": {"apply_archive": True, "archive_target": "s3a", "archive_after_hours": 0}},
        )
        t = add("scenario-d.iso", "arch-d")
        path = watch(t, 90)
        print("   path:", " -> ".join("removed" if p is None else f"{p[0]}[{p[1]}]" for p in path))
        check(s, path[-1] is None, "removed")
        time.sleep(20)
        check(s, "scenario-d.iso" not in bucket(), "nothing left in the bucket")
        leftover = subprocess.run(["ls", "/mnt/a", "/downloads"], capture_output=True, text=True).stdout
        check(s, "scenario-d.iso" not in leftover, "nothing left on the mount or the local disk")

    reset(None, {})
    print()
    if failures:
        print(f"{len(failures)} check(s) failed:")
        for failure in failures:
            print("  -", failure)
        sys.exit(1)
    print("all scenarios passed")


if __name__ == "__main__":
    main()
