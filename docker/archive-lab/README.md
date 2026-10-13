# Archive lab

A local setup for testing archive targets: silo (a MinIO build) as the S3
server, two rclone mounts (`/mnt/a`, `/mnt/b`) and redeluge on top.

Build the binaries first (`docker/rust.sh` builds the image they need), then:

```bash
cargo build --bin redeluged --bin redeluge-web   # inside the redeluge-rust:dev image
docker compose up -d --build
docker compose exec lab python3 /src/docker/archive-lab/lab.py seed
docker compose exec lab python3 /src/docker/archive-lab/lab.py status
```

Web UI on <http://127.0.0.1:18112> (password in `compose.yml`), the S3
console on <http://127.0.0.1:19001>.

`seed` adds two fake ISOs under the labels `isos` (target `s3a`, `/mnt/a`) and
`films` (target `s3b`, `/mnt/b`), both archiving as soon as they finish.

## Outages

| What | How |
|---|---|
| rclone crashes | `docker compose exec lab bash /src/docker/archive-lab/mount.sh kill a` |
| Clean unmount | `docker compose exec lab bash /src/docker/archive-lab/mount.sh down a` |
| Bring it back | `docker compose exec lab bash -c 'fusermount3 -uz /mnt/a; bash /src/docker/archive-lab/mount.sh up a'` |
| Restart during an outage | `LAB_DOWN=a docker compose up -d --force-recreate lab` |

The torrents on that target show **Remote Error** and stay paused at their
progress; they resume on their own when the mount answers again.

## Against a real S3 (MEGA S4, R2, AWS…)

Put a `rclone.conf` next to this file (it is ignored by git) with your remote,
then point target B at it:

```ini
[s4]
type = s3
provider = Other
endpoint = https://<your endpoint>
access_key_id = <key>
secret_access_key = <secret>
```

```bash
REMOTE_B=s4:<bucket> docker compose up -d --force-recreate lab
```

Use keys made for the test, and revoke them afterwards.
