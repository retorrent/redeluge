# Docker

The image builds the daemon and the Web UI from this working tree and runs both
in one container. Two Rust binaries and the C++ libtorrent they drive: no
interpreter, no virtualenv, and the Web UI assets are compiled into the web
binary rather than shipped beside it.

## First run

```bash
cp .env.example .env
$EDITOR .env          # set DELUGE_WEB_PASSWORD
docker compose up -d --build
```

The Web UI is then on <http://127.0.0.1:8112>.

The first boot writes `data/config`, wires the Web UI to the local daemon and
applies your password. You land straight on the torrent list, with no trip
through the connection manager.

## What is where

| Path | Contents |
|---|---|
| `Dockerfile` | Two-stage build: Rust binaries, then a runtime with no toolchain |
| `docker/entrypoint.sh` | Root stage: PUID/PGID, ownership, privilege drop |
| `docker/run.sh` | Starts and supervises `redeluged` and `redeluge-web` |
| `docker/Dockerfile.rust` | The build and test environment, not a release image |
| `data/config` | Container `/config`, the configuration directory |
| `data/downloads` | Container `/downloads` |

## Controlling upgrades

Nothing floats. Two inputs decide what ends up in the image.

1. **Application code** is whatever is checked out here. Rebuild to pick it up.
2. **Dependency versions** are pinned in `Cargo.lock`, and the build uses
   `--locked`, so a rebuild cannot quietly resolve something newer.

libtorrent comes from Debian rather than being vendored: trixie ships 2.0.11,
which is the version Deluge is tested against.

```bash
cargo update --dry-run   # what has moved
```

## Publishing an image

`.github/workflows/docker.yml` builds the image and publishes it to this
repository's package registry. It runs only when someone starts it, from the
Actions tab, because publishing follows a version bump rather than a push.

The tag is the version in `[workspace.package]` of `Cargo.toml` and nothing
else; the workflow reads it and refuses a version that does not look like one.
So the release procedure is one edit:

```toml
[workspace.package]
version = "1.0.3"
```

Then run the workflow. It stops rather than replacing a version already in the
registry, so a tag someone has pulled cannot change under them; re-run with
*overwrite* if replacing it is what you mean.

| Input | |
|---|---|
| `push` | Off builds without publishing, which is how to check that a change still compiles |
| `latest` | Whether `latest` moves to this build |
| `checks` | Runs the same gate as `docker/rust.sh` first; roughly doubles the run |
| `overwrite` | Replace a version already published |
| `platforms` | `linux/amd64`, or both architectures through emulation, which is slow |

The published image carries the usual OCI labels, so the version and the commit
it was built from are readable without pulling it:

```bash
docker buildx imagetools inspect ghcr.io/retorrent/redeluge:1.3.1
```

A locally built image is tagged `redeluge:local` and carries no version,
because there is nothing for it to carry: the one number that means anything is
in `Cargo.toml`, which is what the binaries report and what the publish
workflow reads. Since 1.6.0 it is also the version the daemon reports to
clients, where it used to answer Deluge's `2.2.1`.

## Coming from the Python daemon

The torrent list was a Python pickle, which this daemon cannot read. Convert it
once, from the host:

```bash
python3 tools/migrate_state.py ./data/config
```

Nothing is deleted. The container refuses to start until this is done, because
an empty torrent list looks exactly like having lost everything. The script is
standard library only, and is in the image at
`/usr/local/share/redeluge/migrate_state.py` if you need a copy.

## Environment variables

| Variable | Default | Effect |
|---|---|---|
| `DELUGE_WEB_PASSWORD` | unset | Web UI password, applied on first boot |
| `DELUGE_WEB_PASSWORD_RESET` | `0` | Set to `1` to overwrite the stored password once |
| `DELUGE_WEB_PORT` | `8112` | Web UI port inside the container |
| `DELUGE_WEB_INTERFACE` | `0.0.0.0` | Web UI bind address inside the container |
| `DELUGE_WEB_BASE` | `/` | Path prefix, for serving under a reverse proxy subpath |
| `DELUGE_DAEMON_PORT` | `58846` | Daemon RPC port |
| `DELUGE_LOGLEVEL` | `info` | `none`, `critical`, `error`, `warning`, `info` or `debug`, translated to `RUST_LOG` |
| `RUST_LOG` | unset | What the binaries actually read. Wins over `DELUGE_LOGLEVEL`, and can name a module: `redeluge_daemon::features=debug` |
| `PUID` / `PGID` | `1000` | Ownership of `/config` and `/downloads` |
| `UMASK` | `022` | Umask for both processes |

Settings you change in the Web UI persist. The bootstrap only rewrites
`default_daemon`, `interface`, `port` and `base` on each boot, so that container
configuration stays authoritative over how the UI is served.

## Exposure

The compose file publishes the Web UI on loopback only. That is deliberate. The
Web UI authenticates with one shared password, it has no rate limiting on
attempts, and any session it issues is an admin session. The password is
stored as scrypt rather than the single round of SHA-1 the Python server used,
so an online guess is slow; slow is not none. Put a reverse proxy with TLS and
its own authentication in front of it before it is reachable from anywhere
else.

The daemon RPC port is not published and `allow_remote` is `false` in the seeded
`core.conf`. Enable both only if you need a thin client, and remember that the
RPC listener accepts and buffers messages before authenticating.

Peer traffic on 58946 is the one port that genuinely needs to be reachable.

## With an S3 archive (rclone)

To archive torrents to S3 (MEGA S4, R2, B2, AWS…) and keep seeding from there,
rclone mounts the bucket in its own container and the mount is shared with
redeluge's. See [Features](Features#archiving-to-s3-and-other-mounts) for
what redeluge does with it.

**Linux hosts only.** Sharing a FUSE mount between containers relies on mount
propagation, which Docker Desktop on macOS and Windows does not offer for host
folders. There, use the single-container lab in `docker/archive-lab/` to try
it out.

Once on the host:

```bash
sudo mkdir -p /srv/redeluge/archive /srv/redeluge/rclone-cache
# The directory both containers share has to be a shared mount itself.
sudo mount --bind /srv/redeluge/archive /srv/redeluge/archive
sudo mount --make-rshared /srv/redeluge/archive
```

(The two `mount` lines do not survive a reboot: put them in `/etc/fstab`, as
`/srv/redeluge/archive /srv/redeluge/archive none bind,rshared 0 0`, or in a
systemd unit.)

`./rclone/rclone.conf`, with keys made for this:

```ini
[s4]
type = s3
provider = Other
endpoint = https://s3.<region>.megas4.com
access_key_id = <key>
secret_access_key = <secret>
```

`docker-compose.yml`:

```yaml
services:
  rclone-s4:
    image: rclone/rclone:1.68
    container_name: rclone-s4
    restart: unless-stopped
    cap_add: [SYS_ADMIN]
    devices: [/dev/fuse]
    security_opt: [apparmor:unconfined]
    entrypoint: ["/bin/sh", "-c"]
    command:
      - |
        # A mount left by a crash would make rclone refuse to start.
        fusermount3 -uz /data/s4 2>/dev/null || true
        mkdir -p /data/s4
        exec rclone mount s4:my-bucket /data/s4 \
          --allow-other --uid ${PUID:-1000} --gid ${PGID:-1000} \
          --vfs-cache-mode full --vfs-cache-max-size 50G \
          --cache-dir /cache \
          --rc --rc-addr :5572 --rc-user redeluge --rc-pass ${RCLONE_RC_PASS:?set RCLONE_RC_PASS in .env}
    volumes:
      - ./rclone:/config/rclone
      - /srv/redeluge/rclone-cache:/cache
      - type: bind
        source: /srv/redeluge/archive
        target: /data
        bind: {propagation: rshared}

  redeluge:
    # ...everything from the stock docker-compose.yml, plus:
    depends_on: [rclone-s4]
    volumes:
      - ./data/config:/config
      - ./data/downloads:/downloads
      - type: bind
        source: /srv/redeluge/archive
        target: /mnt/archive
        bind: {propagation: rslave}
```

rclone mounts **inside** the shared directory, at `/data/s4`, and redeluge
sees it at `/mnt/archive/s4`. Mounting straight onto a bind-mounted path
(`/mnt/s4` bound from the host) fails: rclone sees a mount already there and
refuses (`directory already mounted, use --allow-non-empty`). Do not use
`--allow-non-empty` to get past it: it would hide whatever was written there
while the mount was down.

Then in Preferences, *Archive*:

```text
s4 | /mnt/archive/s4 | http://redeluge:<RCLONE_RC_PASS>@rclone-s4:5572
```

and in a label's settings, *Archive*: target `s4`, and how long a torrent has
to be idle before it goes. More buckets are more `rclone-…` services, each
mounting its own `/data/<name>`, all under the same shared directory.

What each piece is for:

| | |
|---|---|
| `rshared` / `rslave` | The mount rclone makes inside its container appears in redeluge's, and reappears after rclone restarts |
| `fusermount3 -uz` first | A crashed rclone leaves a dead mount behind on the host, which the next start would refuse to mount over |
| `--allow-other --uid --gid` | redeluge runs as `PUID`/`PGID` (1000 by default), not as rclone's root |
| `--vfs-cache-mode full` | libtorrent reads and writes at random offsets; without the cache, writes into the mount fail |
| `--vfs-cache-max-size` | Has to hold the largest torrent: a move into the archive is written to the cache, then uploaded. The cache lives on the local disk |
| `--rc …` | Lets redeluge tell *uploading* from *archived*, and show rclone's settings in Preferences, Archive. Optional. The password keeps other containers on the network from driving rclone; it is never written to the log |

redeluge only counts a target as online when its path is on a FUSE or network
filesystem (`fuse.rclone`, NFS, CIFS…) that answers. When rclone is down,
`/mnt/archive/s4` is just an empty directory on the local disk, and it is
treated as offline rather than as an empty archive.

If the rclone container stops or crashes, the torrents on `/mnt/s4` show
*Remote Error* and wait; `restart: unless-stopped` brings rclone back and they
resume without a recheck.

## Behaviour on failure

`docker/run.sh` exits as soon as either process does, so a crashed daemon takes
the container down and `restart: unless-stopped` brings the pair back together.
The healthcheck only probes the Web UI port.

## What is not in the image

Nothing that reads an archive. Block lists are downloaded and unpacked by the
daemon itself, gzip included, so `7zip` and `unrar-free` are not installed.

Peer country flags are not filled in. Deluge looked them up in a
legacy-format `GeoIP.dat`, MaxMind retired that format, and the daemon
reports an empty country rather than pretending otherwise.

No interpreter. `tools/migrate_state.py` is in the image at
`/usr/local/share/redeluge/migrate_state.py` as a file to copy out, not to run
there.

## See also

- [Install](Install) for the source and systemd routes.
- [Configuration](Configuration) for what the seeded `core.conf` contains.
- [Migrating from Deluge](Migrating-from-Deluge) for an existing installation.
