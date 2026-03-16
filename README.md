<!-- SPDX-FileCopyrightText: 2026 Alexander R. Croft -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Rally

Rally is a Rust development helper that reads `rally.toml`, launches all your apps as child processes (not services), and gives you a clean embedded web dashboard so you can rally your services, see what's running, check health, view live logs, and restart or kill processes without console chaos.

For a day-to-day operator view, see [USER_GUIDE.md](USER_GUIDE.md).

## Recent Additions

Recent changes to Rally include:

- `access` labels in `rally.toml`, exposed through `/api/status` and shown in the dashboard, with clickable `http://` and `https://` links.
- `RALLY_CONFIG` support with deterministic config path precedence.
- Runtime `enabled` support with `enabled = false` in TOML, dashboard controls, and reload semantics that reset runtime state back to config.
- Local control commands and API endpoints for `start`, `stop`, `restart`, `enable`, and `disable`.

---

## Features

- **`rally.toml` config** — define any number of apps with command, arguments, optional dashboard access labels, environment variables, working directory, and optional HTTP health checks.
- **Access labels** — optionally show a friendly access URL, port, or operator hint in the dashboard instead of the raw launch command.
- **Enabled flags** — optionally mark apps disabled in `rally.toml`, and toggle them at runtime without changing dependency behavior.
- **Lifecycle hooks** — run `before` prep commands and `after` cleanup commands around each app.
- **Dependency ordering** — declare `depends_on` so services come up and go down in a predictable sequence.
- **ENV interpolation** — use `${VAR}` in commands, args, workdirs, URLs, and env values.
- **Config reload** — reload `rally.toml` from the UI or API without restarting the Rally server.
- **Optional telemetry sink** — pass `--sink http://...` to forward Rally lifecycle events to a ratatouille HTTP sink; if the sink is absent or unavailable, Rally keeps running.
- **Output forwarding** — forward managed app stdout and stderr to the optional sink while still keeping in-memory logs in the dashboard.
- **Watch groups** — optionally watch files, config, or local binaries and restart the affected app after a debounce window.
- **CLI surface** — built-in `--help`, `--version`, `--license`, explicit `--config`, optional `--sink`, and legacy positional config compatibility.
- **Config discovery** — config path precedence is explicit: `--config`, positional path, `RALLY_CONFIG`, then `./rally.toml`.
- **Remote control commands** — `start`, `stop`, `restart`, `enable`, and `disable` connect to an already-running Rally instance instead of starting a new one.
- **Embedded web UI** at `http://127.0.0.1:7700` (configurable) — no external tools needed.
- **Live dashboard** — real-time process state, uptime, PID, restart count, health badge.
- **Operational visibility** — the dashboard `Info` tab shows access details, enabled state, watch status, normalized watch paths, and the last restart reason for each app.
- **Log viewer** — per-process stdout/stderr capture with filter and auto-scroll.
- **Start / Stop / Enable / Disable** — control individual processes from the dashboard or CLI.
- **Auto-restart** — optional `restart_on_exit = true` to keep processes alive.
- **Health checks** — optional HTTP health polling with configurable interval.
- **Graceful shutdown** — Ctrl-C stops all child processes cleanly.

---

## Installation

```
cargo install --path .
```

For crates.io, install the published package as:

```sh
cargo install frogfish-rally
```

The published crate name is `frogfish-rally`, while the installed command remains `rally`.

Or build directly:

```
cargo build --release
./target/release/rally
```

For local release helpers:

```sh
# Increment the patch component in VERSION and sync Cargo.toml package version
make bump

# Increment BUILD, run a release build, copy the binary to dist/<os>-<arch>/bin,
# and package the user guide, README, LICENSE, and example config into dist/<os>-<arch>/
make dist

# Delete Cargo build artifacts under target/
make clean

# Delete Cargo build artifacts and packaged dist output
make distclean

# Sync Cargo.toml to VERSION, increment BUILD, test, commit, tag, and push a release
make release
```

## GitHub Actions

The repository is set up for two GitHub Actions workflows:

- `CI` runs on pushes to `main` and `master`, plus pull requests, and verifies `cargo test` and `cargo build --release` on Linux, macOS, and Windows.
- `Release` builds packaged release bundles for Linux, macOS, and Windows and publishes them to a GitHub Release.

The release workflow is intentionally not tied to every push. You can use it in two ways:

1. Push a tag that matches the current `VERSION`, such as `v5.0.0`.
2. Run the `Release` workflow manually from GitHub Actions. If you do not provide a tag, it defaults to `v<VERSION>` and validates that the tag matches the repo's `VERSION` file.

Each published release bundle includes:

- The platform binary in `bin/`
- `USER_GUIDE.md`
- `README.md`
- `LICENSE`
- `rally.toml.example`

Recommended release flow:

1. Run `make bump` if you are incrementing the version.
2. Run `make release`.

`make release` requires a clean working tree. It will:

- sync `Cargo.toml` package version to `VERSION`
- increment `BUILD`
- run `cargo test`
- create a release commit
- create annotated tag `v<VERSION>`
- push the current branch and tag to `origin`

That push triggers the `Release` GitHub Actions workflow, which builds and publishes the cross-platform release artifacts.

---

## Usage

```
# Start with the default rally.toml in the current directory
rally

# Or point Rally at a config file via environment variable
RALLY_CONFIG=/path/to/my/rally.toml rally

# Or point at a specific file explicitly
rally --config /path/to/my/rally.toml

# Legacy positional config path still works
rally /path/to/my/rally.toml

# Or forward Rally lifecycle events to an optional HTTP sink
rally --sink http://127.0.0.1:9100/ingest

# Or control an existing Rally instance for one app
rally restart api-server
rally disable worker

# Show CLI help
rally --help

# Show version with build metadata from VERSION and BUILD
rally --version

# Show copyright and license summary
rally --license
```

Then open **http://127.0.0.1:7700** in your browser.

## CLI

Rally's command line is intentionally small and explicit:

- `rally` starts using `rally.toml` in the current directory.
- `RALLY_CONFIG=/path/to/rally.toml rally` selects a config file through the environment.
- `rally --config /path/to/rally.toml` selects a config file directly.
- `rally /path/to/rally.toml` remains supported for positional compatibility.
- `rally start APP` asks an existing Rally instance to start one app.
- `rally stop APP` asks an existing Rally instance to stop one app.
- `rally restart APP` asks an existing Rally instance to restart one app.
- `rally enable APP` marks one app enabled at runtime.
- `rally disable APP` marks one app disabled at runtime.
- `rally --sink http://127.0.0.1:9100/ingest` enables best-effort ratatouille forwarding.
- `rally --help` prints the full command reference.
- `rally --version` prints the build version in `VERSION+build.BUILD` form.
- `rally --license` prints the copyright and license summary.

The sink is optional by design. If it is not reachable yet, Rally still starts, supervises processes, and simply drops outbound sink messages until delivery is possible.

Config path precedence is deterministic:

1. `--config FILE`
2. legacy positional config path
3. `RALLY_CONFIG`
4. `./rally.toml`

The remote control commands use that same config resolution to find the running Rally instance, then connect to the configured UI host and port over HTTP.

## Reload, Dependencies, and Hooks

Rally can reload configuration in place through the dashboard or `POST /api/reload` without restarting the Rally web server itself.

`depends_on` is enforced for both startup and shutdown. Dependencies start first, dependents stop first, and invalid dependency graphs are rejected before processes are launched.

`before` hooks run in order and must succeed before Rally starts the app. `after` hooks run in order after the app exits or is stopped. Hook environment inherits the app `env` and can add or override values with `before.env` or `after.env`.

When `--sink` is configured, Rally emits its own lifecycle and process events plus managed app stdout and stderr to a ratatouille-compatible HTTP sink in NDJSON format. If the sink is absent or unreachable, Rally continues running and simply drops that outbound telemetry.

Sink topics currently use this shape:

- `rally:lifecycle` for startup, shutdown, and reload messages
- `rally:process` for process lifecycle and restart messages
- `rally:watch` for watcher setup and file-change messages
- `rally:stdout` and `rally:stderr` for forwarded app output

## Watching and Restart Behavior

`[app.watch]` is optional. Rally watches the configured paths plus any local command path such as `./target/debug/api-server`, debounces rapid changes, and restarts only the affected app.

Watch path normalization is deterministic:

- Relative watch paths are resolved against `workdir` when present, otherwise against Rally's current working directory.
- Relative local command paths such as `./target/debug/api-server` are watched automatically even if `watch.paths` is empty.
- If a watched file does not exist yet but its parent directory does, Rally watches the parent directory non-recursively so future writes can still trigger a restart.
- Directory watches honor `recursive = true`; file watches are always non-recursive.

The dashboard `Info` tab shows whether watching is enabled, the normalized watch paths Rally registered, and the last restart reason observed for that app.

---

## Configuration (`rally.toml`)

```toml
# UI settings (optional)
[ui]
host = "127.0.0.1"
port = 7700

# Define as many [[app]] entries as you like
[[app]]
name    = "api-server"
access  = "http://127.0.0.1:8080"   # optional dashboard label; URLs become clickable links
enabled = true                       # optional; defaults to true
command = "./target/debug/api-server"
args    = ["--port", "8080"]
depends_on = ["database"]
workdir = "."                          # optional, defaults to config file directory
restart_on_exit      = false           # auto-restart when process exits (default: false)
health_url           = "http://localhost:8080/health"  # optional HTTP health check
health_interval_secs = 10             # poll interval in seconds (default: 10)
log_lines            = 500            # lines of log to keep in memory (default: 500)

[app.watch]
paths = ["./config/development.toml", "./migrations"]
recursive = true
debounce_millis = 750

[[app.before]]
command = "cargo"
args    = ["build", "--bin", "api-server"]

[[app.after]]
command = "rm"
args    = ["-f", ".api-server.lock"]

[app.env]
DATABASE_URL = "postgres://localhost/mydb"
LOG_LEVEL    = "debug"
DATA_DIR     = "${HOME}/dev/api-data"

[[app]]
name    = "database"
access  = "postgres://localhost:5432/mydb"
command = "docker"
args    = ["compose", "up", "postgres"]

[[app]]
name    = "worker"
access  = "queue: amqp://localhost"
enabled = false                      # start disabled until enabled later
command = "./target/debug/worker"
args    = ["--concurrency", "4"]
depends_on = ["api-server"]
restart_on_exit = true

[app.env]
QUEUE_URL = "amqp://localhost"
HOST      = "127.0.0.1"
API_URL   = "http://${HOST}:8080"
```

`depends_on = ["database", "api-server"]` starts dependencies first and stops dependents first on shutdown. Rally validates that dependency names exist, are unique, and do not form cycles.

`enabled = false` keeps an app disabled at startup and after config reload. If `enabled` is omitted, Rally treats the app as enabled by default.

The runtime enabled flag is dynamic and per-app. Disabling an app does not walk dependencies or stop dependents automatically; it affects only that app. Reloading the config resets runtime enable state back to the value in `rally.toml`.

`ENV` interpolation uses `${VAR}` syntax. Rally resolves values from the current process environment first, then app and hook `env` entries with deterministic cycle detection and unknown-variable errors.

See [`rally.toml.example`](rally.toml.example) for a full example.

If `access` is set, the dashboard shows that value in the app card instead of the launch command line. This is useful for apps that expose their own embedded UI, local admin page, port, or setup hint and are easier to operate by access point than by startup command.

Values beginning with `http://` or `https://` render as links that open in a new tab. Other values such as `localhost:5432`, `queue: amqp://localhost`, or `admin on port 9090` are shown as plain text.

---

## Web API

The embedded server also exposes a simple JSON API:

| Method | Path                    | Description                    |
|--------|-------------------------|--------------------------------|
| GET    | `/api/status`           | JSON array of all process statuses |
| POST   | `/api/start/:name`      | Start a process if enabled     |
| POST   | `/api/stop/:name`       | Stop a process without disabling it |
| POST   | `/api/kill/:name`       | Terminate a process            |
| POST   | `/api/restart/:name`    | Kill then restart a process    |
| POST   | `/api/enable/:name`     | Mark an app enabled at runtime |
| POST   | `/api/disable/:name`    | Mark an app disabled at runtime |
| POST   | `/api/reload`           | Reload config and restart managed processes |
| POST   | `/api/clear-logs/:name` | Clear captured log buffer      |
| GET    | `/api/events`           | Server-Sent Events stream (live updates) |

---

## License

GPL-3.0-or-later
