# start

A Rust development helper that reads `start.toml`, launches all your apps as child processes (not services), and gives you a clean embedded web dashboard — inspired by HashiCorp Nomad — so you can see what's running, check health, view live logs, and restart/kill processes without console chaos.

---

## Features

- **`start.toml` config** — define any number of apps with command, arguments, environment variables, working directory, and optional HTTP health checks.
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
./target/release/start
```

---

## Usage

```
# Start with the default start.toml in the current directory
start

# Or point at a specific file
start /path/to/my/start.toml
```

Then open **http://127.0.0.1:7700** in your browser.

---

## Configuration (`start.toml`)

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
workdir = "."                          # optional, defaults to config file directory
restart_on_exit      = false           # auto-restart when process exits (default: false)
health_url           = "http://localhost:8080/health"  # optional HTTP health check
health_interval_secs = 10             # poll interval in seconds (default: 10)
log_lines            = 500            # lines of log to keep in memory (default: 500)

[app.env]
DATABASE_URL = "postgres://localhost/mydb"
LOG_LEVEL    = "debug"

[[app]]
name    = "worker"
command = "./target/debug/worker"
args    = ["--concurrency", "4"]
restart_on_exit = true

[app.env]
QUEUE_URL = "amqp://localhost"
```

See [`start.toml.example`](start.toml.example) for a full example.

---

## Web API

The embedded server also exposes a simple JSON API:

| Method | Path                    | Description                    |
|--------|-------------------------|--------------------------------|
| GET    | `/api/status`           | JSON array of all process statuses |
| POST   | `/api/kill/:name`       | Terminate a process            |
| POST   | `/api/restart/:name`    | Kill then restart a process    |
| POST   | `/api/clear-logs/:name` | Clear captured log buffer      |
| GET    | `/api/events`           | Server-Sent Events stream (live updates) |

---

## License

MIT
