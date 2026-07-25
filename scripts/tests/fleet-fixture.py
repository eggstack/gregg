#!/usr/bin/env python3
"""Deterministic HTTP fixture for mixed-fleet release evidence.

Each path represents a stable or transitioning endpoint. Requests and the
observed mode are written as JSON lines so a client harness can reconcile
expected and observed transitions without relying on public hosts.
"""

from __future__ import annotations

import argparse
import json
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


class FixtureHandler(BaseHTTPRequestHandler):
    log_path: Path
    default_mode: str = "healthy"
    calls: dict[str, int] = {}

    def log_message(self, _format: str, *_args: object) -> None:
        return

    def do_GET(self) -> None:  # noqa: N802 - stdlib handler API
        path, _, query = self.path.partition("?")
        path_mode = path.strip("/")
        mode = self.default_mode if path_mode in {"", "v1/status"} else path_mode
        if query:
            for item in query.split("&"):
                key, _, value = item.partition("=")
                if key == "mode" and value:
                    mode = value
        self.calls[mode] = self.calls.get(mode, 0) + 1
        call = self.calls[mode]
        record = {"mode": mode, "call": call, "observed_at": time.time()}
        with self.log_path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(record, sort_keys=True) + "\n")

        if mode in {"slow", "timeout"}:
            time.sleep(0.5 if mode == "slow" else 3.0)
        if mode == "offline" or (mode == "recover" and call == 1):
            self.send_error(503, "offline fixture")
            return
        if mode == "error" or (mode == "healthy-to-failure" and call == 2):
            self.send_error(500, "error fixture")
            return
        if mode == "malformed":
            body = b"{not-json"
            status = 200
        else:
            status = 200
            observed_at = int(time.time() * 1000)
            if mode == "stale":
                observed_at = 1
            body = json.dumps(
                {
                    "schema_version": 1,
                    "observed_at_unix_ms": observed_at,
                    "sample_interval_ms": 1000,
                    "capabilities": {"cpu_iowait": False},
                    "system": {
                        "name": mode,
                        "hostname": "fixture.local",
                        "os_name": "fixture",
                        "os_version": "1",
                        "kernel_name": "fixture",
                        "kernel_release": "1",
                        "architecture": "test",
                    },
                    "cpu": {"logical_cores": 1, "usage_pct": 1.0, "iowait_pct": None},
                    "load": {"one": 0.1, "five": 0.1, "fifteen": 0.1},
                    "memory": {"used_bytes": 1, "total_bytes": 2, "usage_pct": 50.0},
                    "swap": {"used_bytes": 0, "total_bytes": 0, "usage_pct": 0.0},
                }
            ).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", required=True, type=int)
    parser.add_argument("--log", required=True, type=Path)
    parser.add_argument("--default-mode", default="healthy")
    args = parser.parse_args()
    FixtureHandler.log_path = args.log
    FixtureHandler.default_mode = args.default_mode
    args.log.parent.mkdir(parents=True, exist_ok=True)
    server = ThreadingHTTPServer(("127.0.0.1", args.port), FixtureHandler)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
