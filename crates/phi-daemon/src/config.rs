use std::{env, net::SocketAddr};

use thiserror::Error;

pub const BIND_ADDRESS_ENV: &str = "PHI_DAEMON_BIND";
pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:8787";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonConfig {
    bind_address: SocketAddr,
}

impl DaemonConfig {
    pub fn new(bind_address: SocketAddr) -> Self {
        Self { bind_address }
    }

    pub fn from_env() -> Result<Self, ConfigError> {
        let value = match env::var(BIND_ADDRESS_ENV) {
            Ok(value) => value,
            Err(env::VarError::NotPresent) => DEFAULT_BIND_ADDRESS.to_owned(),
            Err(source) => {
                return Err(ConfigError::Environment {
                    name: BIND_ADDRESS_ENV,
                    source,
                });
            }
        };
        let bind_address = value
            .parse()
            .map_err(|source| ConfigError::InvalidBindAddress { value, source })?;
        Ok(Self::new(bind_address))
    }

    pub fn bind_address(&self) -> SocketAddr {
        self.bind_address
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self::new(
            DEFAULT_BIND_ADDRESS
                .parse()
                .expect("the default daemon bind address must be valid"),
        )
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not read environment variable {name}: {source}")]
    Environment {
        name: &'static str,
        #[source]
        source: env::VarError,
    },

    #[error("invalid daemon bind address {value:?}: {source}")]
    InvalidBindAddress {
        value: String,
        #[source]
        source: std::net::AddrParseError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_loopback() {
        let config = DaemonConfig::default();
        assert_eq!(config.bind_address().to_string(), DEFAULT_BIND_ADDRESS);
        assert!(config.bind_address().ip().is_loopback());
    }
}
