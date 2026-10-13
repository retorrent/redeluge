#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Starts the two mounts and redeluge inside the lab container.
set -euo pipefail

conf=/root/.config/rclone/rclone.conf
mkdir -p "$(dirname "$conf")"
if [[ -f /src/docker/archive-lab/rclone.conf ]]; then
    cp /src/docker/archive-lab/rclone.conf "$conf"
fi
# The MinIO remote is always there, appended to whatever was provided.
cat >> "$conf" <<'CONF'
[minio]
type = s3
provider = Minio
endpoint = http://minio:9000
access_key_id = lab
secret_access_key = labpassword
CONF

until rclone lsd minio: >/dev/null 2>&1; do sleep 1; done
rclone mkdir minio:archive-a
rclone mkdir minio:archive-b

mkdir -p /mnt/a /mnt/b /downloads /config /cache
for which in a b; do
    if [[ ${LAB_DOWN:-} == *${which}* ]]; then
        echo "[lab] leaving /mnt/${which} unmounted"
    else
        bash /src/docker/archive-lab/mount.sh up "${which}"
    fi
done

export PATH=/src/target-docker/debug:$PATH
export DELUGE_CONFIG_DIR=/config DELUGE_DOWNLOAD_DIR=/downloads
export DELUGE_WEB_INTERFACE=0.0.0.0 DELUGE_WEB_PORT=8112 DELUGE_WEB_BASE=/
exec bash /src/docker/run.sh
