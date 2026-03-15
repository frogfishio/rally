<!-- SPDX-FileCopyrightText: 2026 Alexander R. Croft -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Rally

Rally is a Rust development helper that reads `rally.toml`, launches all your apps as child processes (not services), and gives you a clean embedded web dashboard so you can rally your services, see what's running, check health, view live logs, and restart or kill processes without console chaos.

---

## Features

- **`rally.toml` config** — define any number of apps with command, arguments, environment variables, working directory, and optional HTTP health checks.
- **Lifecycle hooks** — run `before` prep commands and `after` cleanup commands around each app.
- **Dependency ordering** — declare `depends_on` so services come up and go down in a predictable sequence.
- **ENV interpolation** — use `${VAR}` in commands, args, workdirs, URLs, and env values.
- **Config reload** — reload `rally.toml` from the UI or API without restarting the Rally server.
- **Optional telemetry sink** — pass `--sink http://...` to forward Rally lifecycle events to a ratatouille HTTP sink; if the sink is absent or unavailable, Rally keeps running.
- **Watch groups** — optionally watch files, config, or local binaries and restart the affected app after a debounce window.
- **Embedded web UI** at `http://127.0.0.1:7700` (configurable) — no external tools needed.
- **Live dashboard** — real-time process state, uptime, PID, restart count, health badge.
- **Log viewer** — per-process stdout/stderr capture with filter and auto-scroll.
- **Kill / Restart** — one-click stop or restart of individual processes from the UI.
- **Auto-restart** — optional `restart_on_exit = true` to keep processes alive.
- **Health checks** — optional HTTP health polling with configurable interval.
- **Graceful shutdown** — Ctrl-C stops all child processes cleanly.

---

## Installation

```
cargo install --path .
```

Or build directly:

```
cargo build --release
./target/release/rally
```

---

## Usage

```
# Start with the default rally.toml in the current directory
rally

# Or point at a specific file explicitly
rally --config /path/to/my/rally.toml

# Legacy positional config path still works
rally /path/to/my/rally.toml

# Or forward Rally lifecycle events to an optional HTTP sink
rally --sink http://127.0.0.1:9100/ingest

# Show CLI help
rally --help
```

Then open **http://127.0.0.1:7700** in your browser.

When `--sink` is configured, Rally emits its own lifecycle and process events plus managed app stdout and stderr to a ratatouille-compatible HTTP sink in NDJSON format. If the sink is absent or unreachable, Rally continues running and simply drops that outbound telemetry.

Sink topics currently use this shape:

- `rally:lifecycle` for startup, shutdown, and reload messages
- `rally:process` for process lifecycle and restart messages
- `rally:watch` for watcher setup and file-change messages
- `rally:stdout` and `rally:stderr` for forwarded app output

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
command = "docker"
args    = ["compose", "up", "postgres"]

[[app]]
name    = "worker"
command = "./target/debug/worker"
args    = ["--concurrency", "4"]
depends_on = ["api-server"]
restart_on_exit = true

[app.env]
QUEUE_URL = "amqp://localhost"
HOST      = "127.0.0.1"
API_URL   = "http://${HOST}:8080"
```

`before` hooks run in order and must succeed before Rally starts the app. `after` hooks run in order after the app exits or is stopped. Hook environment inherits the app `env` and can add or override values with `before.env` or `after.env`.

`depends_on = ["database", "api-server"]` starts dependencies first and stops dependents first on shutdown. Rally validates that dependency names exist, are unique, and do not form cycles.

`ENV` interpolation uses `${VAR}` syntax. Rally resolves values from the current process environment first, then app and hook `env` entries with deterministic cycle detection and unknown-variable errors.

`[app.watch]` is optional. Rally watches the configured paths plus any local command path such as `./target/debug/api-server`, debounces rapid changes, and restarts only the affected app.

Watch path normalization is deterministic:

- Relative watch paths are resolved against `workdir` when present, otherwise against Rally's current working directory.
- Relative local command paths such as `./target/debug/api-server` are watched automatically even if `watch.paths` is empty.
- If a watched file does not exist yet but its parent directory does, Rally watches the parent directory non-recursively so future writes can still trigger a restart.
- Directory watches honor `recursive = true`; file watches are always non-recursive.

The dashboard `Info` tab now shows whether watching is enabled, the normalized watch paths Rally registered, and the last restart reason observed for that app.

See [`rally.toml.example`](rally.toml.example) for a full example.

---

## Web API

The embedded server also exposes a simple JSON API:

| Method | Path                    | Description                    |
|--------|-------------------------|--------------------------------|
| GET    | `/api/status`           | JSON array of all process statuses |
| POST   | `/api/kill/:name`       | Terminate a process            |
| POST   | `/api/restart/:name`    | Kill then restart a process    |
| POST   | `/api/reload`           | Reload config and restart managed processes |
| POST   | `/api/clear-logs/:name` | Clear captured log buffer      |
| GET    | `/api/events`           | Server-Sent Events stream (live updates) |

---

## License

GPL-3.0-or-later
