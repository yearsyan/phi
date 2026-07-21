use std::{
    env,
    ffi::OsString,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

use phi_daemon::{config::DaemonConfig, run, telemetry};

#[tokio::main]
async fn main() -> Result<(), phi_daemon::DaemonError> {
    telemetry::init();
    let options = CliOptions::from_args(env::args_os().skip(1));
    let mut config = DaemonConfig::from_env()?.with_qr_enabled(options.qr_enabled);
    if options.lan_enabled {
        let port = config.bind_address().port();
        config = config.with_bind_address(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port));
    }
    run(config).await
}

#[derive(Debug, PartialEq, Eq)]
struct CliOptions {
    qr_enabled: bool,
    lan_enabled: bool,
}

impl CliOptions {
    fn from_args(args: impl IntoIterator<Item = OsString>) -> Self {
        let mut options = Self {
            qr_enabled: true,
            lan_enabled: false,
        };
        for argument in args {
            if argument == "--no-qr" {
                options.qr_enabled = false;
            } else if argument == "--lan" {
                options.lan_enabled = true;
            }
        }
        options
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_control_lan_binding_and_the_connection_qr_code_independently() {
        assert_eq!(
            CliOptions::from_args([OsString::from("--lan"), OsString::from("--no-qr")]),
            CliOptions {
                qr_enabled: false,
                lan_enabled: true,
            }
        );
        assert_eq!(
            CliOptions::from_args([]),
            CliOptions {
                qr_enabled: true,
                lan_enabled: false,
            }
        );
    }
}
