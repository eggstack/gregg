# greggd

[![Crates.io](https://img.shields.io/crates/v/greggd.svg)](https://crates.io/crates/greggd)
[![Docs.rs](https://docs.rs/greggd/badge.svg)](https://docs.rs/greggd)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A lightweight metrics daemon for Linux, macOS, and Windows. `greggd` collects
host telemetry and serves a read-only JSON API on port `11310` by default.

## Install

Prebuilt release binaries are recommended. Linux or macOS:

```sh
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh \
  | sudo bash -s -- greggd
```

For a source install, use Rust 1.89 or newer:

```sh
cargo install greggd --locked
```

See the [installation guide](https://github.com/eggstack/gregg/blob/main/docs/installation.md)
for Windows, platform options, updates, and uninstall instructions.

## Run

```sh
greggd run
greggd status
greggd configprint
```

`greggd run` stays in the foreground. Use `greggd run --config PATH` to select
a configuration file. The default config paths and service setup are described
in the [daemon guide](https://github.com/eggstack/gregg/blob/main/docs/daemon.md).

For automatic startup on Unix, use `greggd startup install`. On Windows, the
PowerShell installer registers the service when run as Administrator. The
Windows `greggd startup install` command reports service state but does not
create the registration.

## Scheduled jobs

Optional `[[jobs]]` entries run local commands on five-field cron schedules.
Commands are argv arrays and run without an implicit shell. Jobs run as the
daemon's OS user, one at a time. Load gates are supported on Unix; Windows
supports time-based jobs only.

Scheduler history is kept in memory and may include bounded command output. The
HTTP listener is unauthenticated, so any host that can reach it can read that
output. Keep the listener on a trusted network and do not include credentials
in command arguments or output. See the
[daemon guide](https://github.com/eggstack/gregg/blob/main/docs/daemon.md#scheduled-maintenance)
for configuration and limits.

Manage local jobs in the selected config with `greggd job add NAME --schedule '0 2 * * *' -- PROGRAM [ARGS...]`, `greggd job list`, and `greggd job remove [NAME]`. The `--` separator preserves the executable and each argument as distinct argv values; no shell is involved. Omitting the remove name opens a terminal selector, while named removal is noninteractive. After an actual successful add/remove write, the CLI makes one config-specific local rehash attempt after releasing the config lock. The saved file is authoritative: if no matching daemon is running, it will load on the next daemon start; a rejection, restart-required diff, busy response, or uncertain acknowledgment reports a nonzero partial outcome without rollback or retry. Failed, duplicate, missing, list, empty, and cancelled paths do not send a request. `greggd edit` opens the complete config using `$VISUAL`, `$EDITOR`, or a platform fallback, edits a private candidate, and commits only after parsing and validation; it does not auto-rehash. All commands honor `--config` and write atomically where applicable. If replacement succeeds but parent-directory synchronization fails, the command reports that the new file is live while crash durability is uncertain; the replacement is not rolled back. Run `greggd [--config PATH] rehash` for an explicit jobs-only apply; mixed changes are rejected atomically. `greggd [--config PATH] configtest` uses the same bounded 1 MiB rehash loader, and, when the matching daemon is reachable, checks daemon-principal job authority and restart-only compatibility without applying or publishing anything. Offline success is local-load-only and says live eligibility was not checked. Changes to `name`, `host`, `port`, sampling/staleness settings, `allow_privileged_jobs`, or `scheduler_history_limit` still require a restart. Rehash and configtest repeat the root-job authority check; `allow_privileged_jobs` is never changed implicitly.

## API and security

The API is read-only. See the
[HTTP API guide](https://github.com/eggstack/gregg/blob/main/docs/api.md) for
routes and optional telemetry. The daemon has no TLS or authentication and is
not intended for public Internet exposure. See the project's
[security guidance](https://github.com/eggstack/gregg/blob/main/SECURITY.md).

## License

MIT
