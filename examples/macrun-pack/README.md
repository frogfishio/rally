# Macrun Sample Pack

This sample pack shows how Rally can load its base environment from macrun without wrapper scripts or repo-local `.env` files.

Seed a temporary macrun scope:

```sh
macrun set \
  --project rally-macrun-sample \
  --profile dev \
  RALLY_SAMPLE_HOST=127.0.0.1 \
  RALLY_SAMPLE_PORT=7811 \
  RALLY_SAMPLE_MESSAGE='hello from macrun'
```

Run Rally with the sample config:

```sh
cargo run -- --config ./examples/macrun-pack/rally.toml
```

Then open:

```text
http://127.0.0.1:7700
```

The sample starts:

- `sample-http`, a local Python HTTP server
- `sample-worker`, a simple shell loop that emits log lines using env values loaded from macrun

Cleanup:

```sh
macrun unset \
  --project rally-macrun-sample \
  --profile dev \
  RALLY_SAMPLE_HOST \
  RALLY_SAMPLE_PORT \
  RALLY_SAMPLE_MESSAGE
```