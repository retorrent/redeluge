#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# mount.sh up|down|kill a|b — start, stop cleanly, or kill -9 one mount.
set -euo pipefail
action=$1 which=$2
case $which in
    a) remote=$REMOTE_A port=5572 ;;
    b) remote=$REMOTE_B port=5573 ;;
    *) echo "a or b" >&2; exit 2 ;;
esac
pidfile=/run/rclone-$which.pid
case $action in
    up)
        rclone mount "$remote" "/mnt/$which" \
            --vfs-cache-mode full --vfs-cache-max-size 2G \
            --vfs-write-back "${LAB_WRITE_BACK:-5s}" --cache-dir "/cache/$which" \
            --rc --rc-addr "127.0.0.1:$port" --rc-no-auth \
            --log-file "/var/log/rclone-$which.log" --log-level INFO &
        echo $! > "$pidfile"
        for _ in $(seq 1 30); do
            mountpoint -q "/mnt/$which" && exit 0
            sleep 0.5
        done
        echo "mount $which did not come up" >&2; exit 1 ;;
    down)
        fusermount3 -u "/mnt/$which" ;;
    kill)
        # What a crash looks like: the mount is left as a dead endpoint.
        kill -9 "$(cat "$pidfile")" ;;
esac
