<!-- SPDX-FileCopyrightText: 2026 Alexander R. Croft -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Rally User Guide

Rally is a local control panel for a group of development apps. You start Rally, open the dashboard in a browser, and use it to see what is running, inspect health, read logs, and restart or stop individual apps when needed.

This guide is for people using Rally day to day. It focuses on what to do, what to expect, and where to look when something is wrong.

---

## What Rally Does

Rally reads a `rally.toml` file and starts the apps listed there.

Once Rally is running, it gives you:

- A browser dashboard showing all managed apps.
- An optional top-level env provider command, so shared packs can pull secrets or profile-scoped values from external tools before Rally resolves `[env]` and `[app.env]`.
- An optional access label for each app, so embedded UIs and local endpoints are easier to recognize.
- Per-app enabled state, so you can keep selected apps disabled without removing them from config.
- Effective env visibility, so you can inspect the final env Rally applies to each app.
- Live status for each app.
- Health indicators when an app has a health check configured.
- Per-app logs.
- Restart and stop controls.
- Config reload without restarting Rally itself.

Rally manages local child processes. It is not a service manager and it does not install anything into your operating system.

---

## Starting Rally

In the directory that contains your `rally.toml` file, run:

```sh
rally
```

If the config file is somewhere else, run:

```sh
rally --config /path/to/rally.toml
```

You can also point Rally at a config file through the environment:

```sh
RALLY_CONFIG=/path/to/rally.toml rally
```

Then open the dashboard in your browser. By default, Rally serves it at:

```text
http://127.0.0.1:7700
```

If your setup uses a different host or port, use the address configured in `rally.toml`.

When more than one config source is available, Rally uses this precedence order:

1. `--config /path/to/rally.toml`
2. positional config path
3. `RALLY_CONFIG`
4. `./rally.toml`

That same config resolution is also how Rally's command-line control commands find an already-running Rally instance on your machine.

---

## First Look at the Dashboard

Each app appears as its own card.

On each card you can usually see:

- The app name.
- Its access point when one is configured, such as a local UI URL, port, or operator hint.
- Whether the app is currently enabled or disabled.
- Whether it is running, stopped, exited, or unhealthy.
- Whether Rally thinks it is already running externally.
- Its process ID while running.
- Restart count.
- Uptime.
- Quick actions such as restart or kill.

When an app is disabled, Rally shows that clearly in the card and Info view. A disabled app is different from an app that merely exited.

If an app shows `external`, Rally found the expected local listener already responding before it launched the process. In practice that usually means the app is already running outside Rally and Rally deliberately did not start a duplicate.

Selecting an app opens more detail, including logs, environment values, and operational information.

The Env tab shows the final environment Rally will apply to that app, including any shared `[env]` values from the top of `rally.toml` plus that app's own `[app.env]` overrides.

If the config uses `[env_command]`, those provider-loaded values are included in the final environment before Rally applies shared and app-specific overrides.

By default, the Env tab shows only managed values that came from Rally config or the env provider. You can switch it to show the full inherited environment when you need to inspect ambient variables passed through from the Rally process.

If an app has its own embedded UI, admin page, or local endpoint, that access point can be shown directly on the card instead of the raw startup command. If the value is an `http://` or `https://` URL, you can open it directly from the dashboard in a new tab.

If Rally needs to install a missing binary through Cargo, the app shows an `installing` state until the install finishes.

The Info tab also shows whether an env provider is active, how many keys it loaded, and when Rally last refreshed that provider state.

---

## Common Tasks

### Restart an App

Use the restart control for the app when you want Rally to stop it and start it again.

This is useful when:

- The app is stuck.
- A watched file changed and you want to force a fresh run.
- You changed something outside the configured watch paths.

### Stop an App

Use the kill control to terminate just that app.

If the app is configured with automatic restart on exit, Rally may bring it back. In that case, killing it is not the same as disabling it permanently.

### Disable an App

Use disable when you want Rally to treat that app as unavailable until you explicitly enable it again.

Disabling an app affects only that one app. Rally does not automatically disable dependents or dependencies.

This is useful for local failure testing, such as disabling a database while leaving an API running so you can observe failure handling.

### Enable an App

Enable restores the app to an active state in Rally, but the runtime enabled flag is still separate from config reload.

If the app was disabled only at runtime, reloading Rally resets it back to whatever `rally.toml` says. If `enabled` was omitted in the config, reload returns that app to the default enabled state.

### Control Rally from Scripts

You can control a running Rally instance without opening the dashboard.

Examples:

```sh
rally start api-server
rally stop api-server
rally restart api-server
rally enable worker
rally disable worker
```

These commands do not start a new Rally server. They resolve the config file, read the configured Rally UI host and port, and send a local HTTP request to the already-running instance.

### Reload Configuration

Use the reload action when `rally.toml` has changed and you want Rally to pick up the new settings.

Reloading configuration does not restart the Rally web dashboard itself, but it can restart managed apps so the new configuration takes effect.

### Read Logs

Open the Logs view for an app to see the stdout and stderr output Rally has captured.

Use this first when an app:

- Fails to start.
- Keeps restarting.
- Reports unhealthy.
- Looks idle when it should be doing work.

### Inspect Effective Environment

Open the Env view for an app when you need to confirm which environment variables are actually in play.

Rally shows the final merged environment, not just the app-local overrides. That is useful when some values come from a shared top-level `[env]` block and others come from `[app.env]`.

### Check Why Something Restarted

Open the Info view for the app.

Rally shows the last restart reason there, along with the configured access value, watch status, and the watch paths it registered.

The Info view also shows whether the app is currently enabled.

### Open an App's Own UI

Some apps managed by Rally also serve their own local UI.

When the app config includes an `access` value such as `http://127.0.0.1:3000`, Rally shows that in the app card instead of the raw command line.

If the value starts with `http://` or `https://`, select it from the dashboard to open that app's UI in a new browser tab.

If the value is not a web URL, Rally still shows it as a plain label so you can remember details such as a port, local socket, or setup note.

---

## Status Meanings

The exact wording can vary by app state, but these are the main states to expect:

- `running`: the app process is active.
- `pending`: Rally intends to run the app, but it is not yet fully running.
- `installing`: Rally is running `cargo install` because the command was not reachable.
- `disabled`: Rally will not start the app until it is enabled again.
- `exited`: the app ran and then ended.
- `killed`: the app was terminated explicitly.
- `failed`: Rally could not start the app successfully.
- `unhealthy`: the app process may still be running, but its configured health check is failing.

An unhealthy app is often still alive as a process. It usually means the app is not responding correctly on its health endpoint.

`stop` and `kill` are also different from `disable`. Stopping or killing affects the current process instance. Disabling changes Rally's runtime intent for that app.

---

## Health Checks

Some apps have a health check configured. When they do, Rally polls the configured URL and shows the result in the dashboard.

If an app shows unhealthy:

1. Open its logs.
2. Check whether the app has fully started yet.
3. Confirm the expected port or URL is correct.
4. Restart the app if needed.

If the app is actually working but still shows unhealthy, the configured health URL may be wrong or too strict for that app.

Health checks and `access` serve different purposes. A health check tells Rally whether the app responds correctly. `access` tells you where or how to reach the app as an operator.

---

## Watched Files and Automatic Restarts

Some apps are configured to restart automatically when files or directories change.

If watching is enabled for an app, Rally can restart it after changes such as:

- A local binary being rebuilt.
- A config file being updated.
- A watched directory receiving new or changed files.

Rally uses a debounce delay, so rapid bursts of file changes are grouped together instead of causing repeated restarts immediately.

If an app is restarting unexpectedly, check the Info view first. It shows whether watching is enabled and which paths Rally is watching.

---

## When an App Does Not Start

Start with these checks:

1. Open the app logs in the dashboard.
2. Confirm the command or binary actually exists on your machine.
3. Confirm any required dependencies are available.
4. Reload the config if `rally.toml` has changed.
5. Restart the app manually from the dashboard.

Possible reasons include:

- The executable path is wrong.
- The app needs environment variables that are missing.
- A `before` hook failed.
- A dependency app failed to start first.
- The app is currently disabled.
- The app exits immediately by design or due to an error.

---

## When Rally Starts but Nothing Happens

If the dashboard opens but no apps are running:

1. Confirm the `rally.toml` file contains `[[app]]` entries.
2. Make sure you started Rally in the directory you expected, or used `--config`.
3. Reload the config after any changes.
4. Check the terminal where Rally itself was started for startup messages.

---

## Configuring Access Labels

In `rally.toml`, each `[[app]]` entry can optionally define an `access` field.

Examples:

```toml
[[app]]
name = "frontend"
access = "http://127.0.0.1:3000"
command = "npm"
args = ["run", "dev"]

[[app]]
name = "database"
access = "postgres://localhost:5432/mydb"
command = "docker"
args = ["compose", "up", "postgres"]

[[app]]
name = "admin-worker"
access = "admin on port 9090"
command = "./worker"
```

Use `access` when the thing you need to remember is how to reach the app, not how Rally launched it.

---

## Configuring Enabled State

Each app can also set `enabled` in `rally.toml`.

```toml
[[app]]
name = "database"
enabled = true
command = "docker"
args = ["compose", "up", "postgres"]

[[app]]
name = "worker"
enabled = false
command = "./worker"
```

If `enabled` is omitted, Rally defaults it to `true`.

Runtime enable and disable actions affect only the current Rally session. Reloading the config or restarting Rally restores the config-defined value, or `true` when the field is omitted.

This is intentional: runtime toggles are meant for local operator control and testing, while `rally.toml` remains the canonical source for startup behavior.

---

## Configuring Shared Environment Variables

You can define shared environment variables once at the top level of `rally.toml`.

```toml
[env]
HOST = "127.0.0.1"
LOG_LEVEL = "info"

[[app]]
name = "api"
command = "./api"

[app.env]
LOG_LEVEL = "debug"
PORT = "8080"
API_URL = "http://${HOST}:${PORT}"
```

In that example, the app receives `HOST=127.0.0.1`, `LOG_LEVEL=debug`, `PORT=8080`, and `API_URL=http://127.0.0.1:8080`.

Use top-level `[env]` for shared values and `[app.env]` for per-app overrides.

---

## Configuring Cargo Auto-Install

You can optionally add a `cargo` field to an app.

```toml
[[app]]
name = "worker"
cargo = "frogfish-worker"
command = "worker"
```

If Rally cannot reach `command` when starting that app and `cargo` is set, Rally runs `cargo install <target>`, shows the app as `installing`, and then retries the app launch once after the install completes.

Use this when you want Rally to bootstrap runtime tools that are distributed as Cargo binaries but are not guaranteed to be installed ahead of time.

---

## Useful Commands

```sh
# Start with the default rally.toml in the current directory
rally

# Start with a config path from the environment
RALLY_CONFIG=/path/to/rally.toml rally

# Start with an explicit config file
rally --config /path/to/rally.toml

# Control an existing Rally instance
rally restart api-server
rally disable worker

# Show help
rally --help

# Show version and build number
rally --version

# Show copyright and license summary
rally --license
```

If your environment uses an optional sink endpoint for telemetry forwarding, you may also see Rally started with `--sink URL`. You do not need that flag unless your setup specifically uses it.

---

## What Rally Will Not Do

Rally is intentionally focused on local process supervision.

It does not:

- Replace your deployment system.
- Install background services into the OS.
- Keep logs forever.
- Fix application-level errors automatically.

If an app is broken, Rally helps you see that clearly and restart it, but the app still needs to be fixed at the source.

---

## Quick Routine

For normal daily use, the typical flow is:

1. Run `rally`.
2. Open the dashboard.
3. Confirm your apps are running and healthy.
4. Use logs when something looks wrong.
5. Use restart for individual apps.
6. Use reload when `rally.toml` changes.

That is the core loop for operating Rally day to day.