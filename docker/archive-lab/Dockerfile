# SPDX-License-Identifier: GPL-3.0-or-later
# The archive lab: redeluge's build environment plus rclone and FUSE.
# The binaries come from target-docker, built by docker/rust.sh's image.
FROM rclone/rclone:1.68 AS rclone
FROM redeluge-rust:dev
RUN apt-get update \
    && apt-get install -y --no-install-recommends fuse3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=rclone /usr/local/bin/rclone /usr/local/bin/rclone
