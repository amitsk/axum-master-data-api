use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::config::LogFormat;

/// Installs the global tracing subscriber. Panics if one is already installed,
/// so it must be called at most once per process.
pub fn init(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn"));
    let registry = tracing_subscriber::registry().with(filter);
    match format {
        LogFormat::Pretty => registry.with(fmt::layer()).init(),
        LogFormat::Json => registry.with(fmt::layer().json().flatten_event(true)).init(),
    }
}
