use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use secrecy::SecretString;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
    pub log: LogConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub request_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    pub url: SecretString,
    pub max_connections: u32,
    pub acquire_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct AuthConfig {
    pub jwt_secret: SecretString,
    pub write_scope: String,
}

#[derive(Debug, Deserialize)]
pub struct LogConfig {
    pub format: LogFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Pretty,
    Json,
}

impl AppConfig {
    /// Precedence, low to high: config/default.toml, config/{APP_ENV}.toml, APP_* env vars.
    ///
    /// `figment::Error` is a large enum, so this trips `clippy::result_large_err`. The
    /// signature is part of the public contract and config is read exactly once at
    /// startup, so boxing it would cost more than it saves.
    #[allow(clippy::result_large_err)]
    pub fn load() -> Result<Self, figment::Error> {
        let env_name = std::env::var("APP_ENV").unwrap_or_else(|_| "development".to_string());
        Figment::new()
            .merge(Toml::file("config/default.toml"))
            .merge(Toml::file(format!("config/{env_name}.toml")))
            .merge(Env::prefixed("APP_").split("__"))
            .extract()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::Jail;
    use secrecy::ExposeSecret;

    #[test]
    fn env_overrides_toml_and_secrets_come_from_env() {
        Jail::expect_with(|jail| {
            jail.create_dir("config")?;
            jail.create_file(
                "config/default.toml",
                r#"
                [server]
                host = "0.0.0.0"
                port = 8080
                request_timeout_secs = 30
                [database]
                max_connections = 10
                acquire_timeout_secs = 5
                [auth]
                write_scope = "masterdata:write"
                [log]
                format = "pretty"
                "#,
            )?;
            jail.set_env("APP_SERVER__PORT", "9090");
            jail.set_env("APP_DATABASE__URL", "postgres://u:p@h/db");
            jail.set_env("APP_AUTH__JWT_SECRET", "s3cr3t");
            jail.set_env("APP_LOG__FORMAT", "json");
            let cfg = AppConfig::load().expect("load");
            assert_eq!(cfg.server.port, 9090);
            assert_eq!(cfg.database.url.expose_secret(), "postgres://u:p@h/db");
            assert_eq!(cfg.auth.jwt_secret.expose_secret(), "s3cr3t");
            assert_eq!(cfg.log.format, LogFormat::Json);
            Ok(())
        });
    }

    #[test]
    fn missing_secret_is_an_error_naming_the_field() {
        Jail::expect_with(|jail| {
            jail.create_dir("config")?;
            jail.create_file(
                "config/default.toml",
                r#"
                [server]
                host = "0.0.0.0"
                port = 8080
                request_timeout_secs = 30
                [database]
                max_connections = 10
                acquire_timeout_secs = 5
                [auth]
                write_scope = "masterdata:write"
                [log]
                format = "pretty"
                "#,
            )?;
            jail.set_env("APP_DATABASE__URL", "postgres://u:p@h/db");
            let err = AppConfig::load().expect_err("should fail");
            assert!(err.to_string().contains("jwt_secret"), "{err}");
            Ok(())
        });
    }
}
