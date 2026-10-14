# SPDX-License-Identifier: GPL-3.0-or-later
"""Drives the archive lab through the Web UI's JSON API.

    python3 lab.py seed      # two fake ISOs, two labels, two targets
    python3 lab.py status    # where each torrent is, as redeluge says
    python3 lab.py remove N  # remove torrent N with its files

Standard library only. Runs inside the lab container.
"""

import base64
import hashlib
import http.cookiejar
import json
import os
import sys
import urllib.request

URL = os.environ.get("LAB_URL", "http://127.0.0.1:8112/json")
jar = http.cookiejar.CookieJar()
opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
_id = 0


def call(method, *params):
    global _id
    _id += 1
    body = json.dumps({"method": method, "params": list(params), "id": _id}).encode()
    request = urllib.request.Request(
        URL, body, {"Content-Type": "application/json"}
    )
    answer = json.loads(opener.open(request).read())
    if answer.get("error"):
        raise SystemExit(f"{method}: {answer['error']}")
    return answer["result"]


def login():
    call("auth.login", "deluge")
    if not call("web.connected"):
        host = call("web.get_hosts")[0][0]
        call("web.connect", host)


def bencode(value):
    if isinstance(value, int):
        return b"i%de" % value
    if isinstance(value, str):
        value = value.encode()
    if isinstance(value, bytes):
        return b"%d:%s" % (len(value), value)
    if isinstance(value, dict):
        items = sorted((k.encode() if isinstance(k, str) else k, v) for k, v in value.items())
        return b"d" + b"".join(bencode(k) + bencode(v) for k, v in items) + b"e"
    raise TypeError(value)


def make_torrent(name, size):
    """A single-file torrent over random bytes, written into /downloads."""
    piece = 256 * 1024
    data = os.urandom(size)
    os.makedirs("/downloads", exist_ok=True)
    with open(f"/downloads/{name}", "wb") as out:
        out.write(data)
    pieces = b"".join(
        hashlib.sha1(data[i : i + piece]).digest() for i in range(0, size, piece)
    )
    return bencode(
        {
            "announce": "http://127.0.0.1:1/announce",
            "info": {"name": name, "length": size, "piece length": piece, "pieces": pieces},
        }
    )


def seed():
    login()
    call(
        "core.set_config",
        {
            "archive": {
                "targets": [
                    {"name": "s3a", "path": "/mnt/a", "rc_url": "http://127.0.0.1:5572"},
                    {"name": "s3b", "path": "/mnt/b", "rc_url": "http://127.0.0.1:5573"},
                ]
            }
        },
    )
    for label, target in (("isos", "s3a"), ("films", "s3b")):
        try:
            call("label.add", label)
        except SystemExit:
            pass
        call(
            "label.set_options",
            label,
            {"apply_archive": True, "archive_target": target, "archive_after_hours": 0},
        )
    for name, label in (("ubuntu-lab.iso", "isos"), ("debian-lab.iso", "films")):
        blob = make_torrent(name, 24 * 1024 * 1024)
        torrent = call(
            "core.add_torrent_file",
            f"{name}.torrent",
            base64.b64encode(blob).decode(),
            {"download_location": "/downloads"},
        )
        call("label.set_torrent", torrent, label)
        print(name, torrent)


def status():
    login()
    found = call(
        "core.get_torrents_status",
        {},
        ["name", "state", "save_path", "archive_state", "label", "progress", "message"],
    )
    for number, (torrent, row) in enumerate(sorted(found.items()), 1):
        print(
            f"{number} {row['name']:<16} {row['state']:<11} {row['archive_state'] or 'local':<10} "
            f"{row['save_path']:<12} {row['progress']:.0f}% {row['message']}"
        )


def port():
    login()
    print(call("core.get_listen_port"))


def peer(info_hash, host, listen_port):
    """Run on the peer: fetch a torrent from the lab's daemon only."""
    import socket

    login()
    torrent = call("core.add_torrent_magnet", f"magnet:?xt=urn:btih:{info_hash}", {})
    address = socket.gethostbyname(host)
    call("core.connect_peer", torrent, address, int(listen_port))
    print("peer added", torrent, "from", address, listen_port)


def remove(number):
    login()
    found = sorted(call("core.get_torrents_status", {}, ["name"]).items())
    torrent, row = found[int(number) - 1]
    print("removing", row["name"], call("core.remove_torrent", torrent, True))


if __name__ == "__main__":
    commands = {"seed": seed, "status": status, "remove": remove, "port": port, "peer": peer}
    commands[sys.argv[1]](*sys.argv[2:])
