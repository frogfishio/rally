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

# Or point at a specific file
rally /path/to/my/rally.toml

# Or forward Rally lifecycle events to an optional HTTP sink
rally --sink http://127.0.0.1:9100/ingest
```

Then open **http://127.0.0.1:7700** in your browser.

When `--sink` is configured, Rally emits its own lifecycle and process events to a ratatouille-compatible HTTP sink in NDJSON format. Managed app stdout and stderr are still kept local for now; forwarding those remains a separate feature.

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

MIT
