use crate::connection::LogSettings;
use std::time::Instant;

// Yes these look silly. `tracing` doesn't currently support dynamic levels
// https://github.com/tokio-rs/tracing/issues/372
macro_rules! tracing_dynamic_enabled {
    (target: $target:expr, $level:expr) => {{
        use ::tracing::Level;

        match $level {
            Level::ERROR => ::tracing::enabled!(target: $target, Level::ERROR),
            Level::WARN => ::tracing::enabled!(target: $target, Level::WARN),
            Level::INFO => ::tracing::enabled!(target: $target, Level::INFO),
            Level::DEBUG => ::tracing::enabled!(target: $target, Level::DEBUG),
            Level::TRACE => ::tracing::enabled!(target: $target, Level::TRACE),
        }
    }};
}

macro_rules! tracing_dynamic_event {
    (target: $target:expr, $level:expr, $($args:tt)*) => {{
        use ::tracing::Level;

        match $level {
            Level::ERROR => ::tracing::event!(target: $target, Level::ERROR, $($args)*),
            Level::WARN => ::tracing::event!(target: $target, Level::WARN, $($args)*),
            Level::INFO => ::tracing::event!(target: $target, Level::INFO, $($args)*),
            Level::DEBUG => ::tracing::event!(target: $target, Level::DEBUG, $($args)*),
            Level::TRACE => ::tracing::event!(target: $target, Level::TRACE, $($args)*),
        }
    }};
}

fn level_filter_to_levels(filter: log::LevelFilter) -> Option<(tracing::Level, log::Level)> {
    let tracing_level = match filter {
        log::LevelFilter::Error => Some(tracing::Level::ERROR),
        log::LevelFilter::Warn => Some(tracing::Level::WARN),
        log::LevelFilter::Info => Some(tracing::Level::INFO),
        log::LevelFilter::Debug => Some(tracing::Level::DEBUG),
        log::LevelFilter::Trace => Some(tracing::Level::TRACE),
        log::LevelFilter::Off => None,
    };

    tracing_level.zip(filter.to_level())
}

pub use sqlformat;

pub struct QueryLogger<'q> {
    sql: &'q str,
    rows_returned: u64,
    rows_affected: u64,
    start: Instant,
    settings: LogSettings,
}

impl<'q> QueryLogger<'q> {
    pub fn new(sql: &'q str, settings: LogSettings) -> Self {
        Self {
            sql,
            rows_returned: 0,
            rows_affected: 0,
            start: Instant::now(),
            settings,
        }
    }

    pub fn increment_rows_returned(&mut self) {
        self.rows_returned += 1;
    }

    pub fn increase_rows_affected(&mut self, n: u64) {
        self.rows_affected += n;
    }

    pub fn finish(&self) {
        let elapsed = self.start.elapsed();

        let lvl = if elapsed >= self.settings.slow_statements_duration {
            self.settings.slow_statements_level
        } else {
            self.settings.statements_level
        };

        if let Some((tracing_level, log_level)) = level_filter_to_levels(lvl) {
            // The enabled level could be set from either tracing world or log world, so check both
            // to see if logging should be enabled for our level
            let log_is_enabled = tracing_dynamic_enabled!(target: "sqlx::query", tracing_level)
                || log::log_enabled!(target: "sqlx::query", log_level);
            if log_is_enabled {
                let mut summary = parse_query_summary(&self.sql);

                let sql = if summary != self.sql {
                    summary.push_str(" …");
                    format!(
                        "\n\n{}\n",
                        sqlformat::format(
                            &self.sql,
                            &sqlformat::QueryParams::None,
                            sqlformat::FormatOptions::default()
                        )
                    )
                } else {
                    String::new()
                };

                let message = format!(
                    "{}; rows affected: {}, rows returned: {}, elapsed: {:.3?}{}",
                    summary, self.rows_affected, self.rows_returned, elapsed, sql
                );

                tracing_dynamic_event!(target: "sqlx::query", tracing_level, message);
            }
        }
    }
}

impl<'q> Drop for QueryLogger<'q> {
    fn drop(&mut self) {
        self.finish();
    }
}

pub fn parse_query_summary(sql: &str) -> String {
    // For now, just take the first 4 words
    sql.split_whitespace()
        .take(4)
        .collect::<Vec<&str>>()
        .join(" ")
}
