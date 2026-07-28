#!/usr/bin/env python3
"""Bounded loopback sparse-registry fixture for release qualification tests."""

from __future__ import annotations

import hashlib
import http.server
import json
import shutil
import threading
from pathlib import Path


def sparse_index_path(crate: str) -> Path:
    """Return Cargo's lowercase sparse-index path for one crate name."""
    name = crate.lower()
    if len(name) == 1:
        return Path("1") / name
    if len(name) == 2:
        return Path("2") / name
    if len(name) == 3:
        return Path("3") / name[0] / name
    return Path(name[:2]) / name[2:4] / name


class LocalSparseRegistry:
    """Serve one immutable crate from an ephemeral loopback HTTP endpoint."""

    def __init__(self, root: Path, crate_archive: Path, *, crate: str, version: str) -> None:
        self.root = root
        self.crate_archive = crate_archive
        self.crate = crate
        self.version = version
        self._server: http.server.ThreadingHTTPServer | None = None
        self._thread: threading.Thread | None = None

    def start(self) -> str:
        self.root.mkdir(parents=True, exist_ok=True)
        fixture = self

        class Handler(http.server.SimpleHTTPRequestHandler):
            def __init__(self, *args: object, **kwargs: object) -> None:
                super().__init__(*args, directory=str(fixture.root), **kwargs)

            def log_message(self, *_args: object) -> None:
                pass

        self._server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        port = self._server.server_address[1]
        base = f"http://127.0.0.1:{port}"
        download = self.root / "api" / "v1" / "crates" / self.crate / self.version / "download"
        download.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(self.crate_archive, download)
        checksum = hashlib.sha256(self.crate_archive.read_bytes()).hexdigest()
        (self.root / "config.json").write_text(
            json.dumps({"dl": f"{base}/api/v1/crates", "api": f"{base}/api"}) + "\n",
            encoding="utf-8",
        )
        index = self.root / sparse_index_path(self.crate)
        index.parent.mkdir(parents=True, exist_ok=True)
        index.write_text(
            json.dumps({
                "name": self.crate,
                "vers": self.version,
                "deps": [],
                "cksum": checksum,
                "features": {},
                "yanked": False,
                "links": None,
            }, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        return f"sparse+{base}/"

    def write_cargo_home(self, cargo_home: Path, source: str) -> None:
        cargo_home.mkdir(parents=True, exist_ok=True)
        (cargo_home / "config.toml").write_text(
            '[registries.phase34-local-registry]\n'
            f'index = "{source}"\n',
            encoding="utf-8",
        )

    def shutdown(self) -> None:
        if self._server:
            self._server.shutdown()
            self._server.server_close()
        if self._thread:
            self._thread.join(timeout=5)
            if self._thread.is_alive():
                raise RuntimeError("local sparse registry did not shut down")

    def __enter__(self) -> "LocalSparseRegistry":
        self.start()
        return self

    def __exit__(self, *_args: object) -> None:
        self.shutdown()
