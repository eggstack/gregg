#!/usr/bin/env python3
"""Small deterministic HTTP child used by verify-installed-daemon tests."""

import json
import os
import re
import signal
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def config_port(path: str) -> int:
    text = open(path, encoding="utf-8").read()
    match = re.search(r"^port\s*=\s*(\d+)\s*$", text, re.MULTILINE)
    if match is None:
        raise SystemExit("config did not contain flat port field")
    return int(match.group(1))


STATUS = {
    "schema_version": 2,
    "observed_at_unix_ms": 1,
    "sample_interval_ms": 1000,
    "capabilities": {
        "cpu_iowait": True,
        "load_average": True,
        "swap": True,
        "memory_commit": False,
    },
    "system": {
        "name": "loopback-test",
        "hostname": "fake-host",
        "os_name": "linux",
        "os_version": "test",
        "kernel_name": "Linux",
        "kernel_release": "test",
        "architecture": "x86_64",
    },
    "cpu": {"logical_cores": 1, "usage_pct": 1.0, "iowait_pct": 0.0},
    "load": {"one": 0.0, "five": 0.0, "fifteen": 0.0},
    "memory": {"used_bytes": 1, "total_bytes": 2, "usage_pct": 50.0},
    "swap": {"used_bytes": 0, "total_bytes": 0, "usage_pct": 0.0},
    "commit": None,
}


class Handler(BaseHTTPRequestHandler):
    mode = "success"

    def do_GET(self) -> None:  # noqa: N802
        if self.path == "/healthz":
            body = b'{"schema_version":1,"state":"ready"}'
            status = 200
        elif self.path == "/v2/healthz":
            body = b'{"schema_version":2,"state":"ready"}'
            status = 200
        elif self.path == "/v2/status":
            if self.mode == "malformed":
                body = b"{"  # Deliberately malformed JSON.
            else:
                body = json.dumps(STATUS).encode()
            status = 200
        else:
            body = b"not found"
            status = 404
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args: object) -> None:
        return


def main() -> int:
    mode = os.environ.get("FAKE_MODE", "startup")
    if mode == "startup":
        print("fake startup failure", file=sys.stderr)
        return 9
    if mode == "timeout":
        signal.pause()
        return 0

    config = sys.argv[sys.argv.index("--config") + 1]
    server = ThreadingHTTPServer(("127.0.0.1", config_port(config)), Handler)
    Handler.mode = mode

    def terminate(_signum: int, _frame: object) -> None:
        raise SystemExit(7 if mode == "nonzero" else 0)

    signal.signal(signal.SIGTERM, terminate)
    print("fake daemon ready", flush=True)
    try:
        server.serve_forever()
    except SystemExit as error:
        return int(error.code)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
