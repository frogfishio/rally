// SPDX-FileCopyrightText: 2026 Alexander R. Croft
// SPDX-License-Identifier: GPL-3.0-or-later

use ratatouille::{Format, HttpSink, HttpSinkConfig, Logger, LoggerConfig, SourceIdentity};
use std::sync::{Arc, Mutex};
use tracing::warn;

enum SinkState {
    Disabled,
    Enabled(Arc<Mutex<Logger<HttpSink>>>),
}

pub struct TelemetrySink {
    state: SinkState,
}

impl TelemetrySink {
    pub fn new(url: Option<String>) -> Self {
        let Some(url) = url else {
            return Self {
                state: SinkState::Disabled,
            };
        };

        let sink = match HttpSink::new(HttpSinkConfig {
            url,
            token: None,
            user_agent: Some("rally/0.1.0".to_owned()),
        }) {
            Ok(sink) => sink,
            Err(error) => {
                warn!(error = %error, "Failed to configure telemetry sink; continuing without sink");
                return Self {
                    state: SinkState::Disabled,
                };
            }
        };

        let logger = Logger::with_sink(
            LoggerConfig {
                filter: Some("rally*".to_owned()),
                format: Format::Ndjson,
                source: SourceIdentity {
                    app: Some("rally".to_owned()),
                    r#where: Some("rust".to_owned()),
                    instance: std::env::var("HOSTNAME").ok(),
                },
                ..LoggerConfig::default()
            },
            sink,
        );

        Self {
            state: SinkState::Enabled(Arc::new(Mutex::new(logger))),
        }
    }

    pub fn emit(&self, topic: &str, message: String) {
        let SinkState::Enabled(logger) = &self.state else {
            return;
        };

        let logger = Arc::clone(logger);
        let topic = topic.to_owned();
        tokio::task::spawn_blocking(move || {
            if let Ok(mut logger) = logger.lock() {
                let _ = logger.log(&topic, &message);
            }
        });
    }

    pub fn emit_process_output(&self, app_name: &str, stream: &str, line: &str) {
        self.emit(
            match stream {
                "stdout" => "rally:stdout",
                "stderr" => "rally:stderr",
                _ => "rally:output",
            },
            format!("app={} {}", app_name, line),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::TelemetrySink;

    #[test]
    fn disabled_sink_ignores_process_output() {
        let sink = TelemetrySink::new(None);
        sink.emit_process_output("api", "stdout", "hello");
    }
}