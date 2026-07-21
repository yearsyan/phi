use std::{
    io::{self, IsTerminal, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
};

use qrcode::{EcLevel, QrCode, render::unicode};
use serde::Serialize;
use tracing::warn;

use crate::config::DaemonConfig;

const CONNECTION_TYPE: &str = "phi-daemon";
const CONNECTION_VERSION: u8 = 1;
const QR_COLORS: &str = "\x1b[30;47m";
const RESET_COLORS: &str = "\x1b[0m";
const IPV4_ROUTE_PROBE: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 9);
const IPV6_ROUTE_PROBE: SocketAddr = SocketAddr::new(
    IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
    9,
);

#[derive(Serialize)]
struct ConnectionPayload<'a> {
    r#type: &'static str,
    version: u8,
    base_url: String,
    auth_key: &'a str,
}

pub(crate) fn print_for_terminal(config: &DaemonConfig, address: SocketAddr, tls: bool) {
    if !config.qr_enabled() || !io::stderr().is_terminal() {
        return;
    }

    let listener_address = address;
    let address = advertised_address(listener_address);
    match render(config.auth_key(), address, tls) {
        Ok(qr) => {
            let mut output = format!(
                "\nPhi app connection QR (contains the daemon key; keep it private)\n{QR_COLORS}{qr}{RESET_COLORS}\n{}\n",
                base_url(address, tls)
            );
            if listener_address.ip().is_loopback() {
                output.push_str(
                    "Loopback-only listener: restart with --lan for a phone on the local network.\n",
                );
            }
            if listener_address.ip().is_unspecified() && address.ip().is_loopback() {
                output.push_str(
                    "No private LAN IPv4 address was found; set PHI_DAEMON_BIND to a specific reachable address.\n",
                );
            }
            let result = {
                let mut stderr = io::stderr().lock();
                stderr.write_all(output.as_bytes())
            };
            if let Err(error) = result {
                warn!(%error, "could not write daemon connection QR code to the terminal");
            }
        }
        Err(error) => {
            warn!(%error, "could not render daemon connection QR code");
        }
    }
}

fn render(auth_key: &str, address: SocketAddr, tls: bool) -> Result<String, QrError> {
    let payload = ConnectionPayload {
        r#type: CONNECTION_TYPE,
        version: CONNECTION_VERSION,
        base_url: base_url(address, tls),
        auth_key,
    };
    let payload = serde_json::to_vec(&payload).map_err(QrError::Serialize)?;
    let code = QrCode::with_error_correction_level(payload, EcLevel::L).map_err(QrError::Encode)?;
    Ok(code.render::<unicode::Dense1x2>().quiet_zone(true).build())
}

fn base_url(address: SocketAddr, tls: bool) -> String {
    let scheme = if tls { "https" } else { "http" };
    format!("{scheme}://{address}")
}

fn advertised_address(address: SocketAddr) -> SocketAddr {
    advertised_address_with(address, route_ip)
}

fn advertised_address_with(
    address: SocketAddr,
    resolve_route: impl FnOnce(IpAddr, SocketAddr) -> Option<IpAddr>,
) -> SocketAddr {
    if !address.ip().is_unspecified() {
        return address;
    }

    let ip = match address.ip() {
        IpAddr::V4(_) => resolve_route(IpAddr::V4(Ipv4Addr::UNSPECIFIED), IPV4_ROUTE_PROBE)
            .filter(|ip| is_private_ipv4(*ip))
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        IpAddr::V6(_) => resolve_route(IpAddr::V6(Ipv6Addr::UNSPECIFIED), IPV6_ROUTE_PROBE)
            .unwrap_or(IpAddr::V6(Ipv6Addr::LOCALHOST)),
    };
    SocketAddr::new(ip, address.port())
}

fn is_private_ipv4(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V4(ip) if ip.is_private())
}

fn route_ip(bind_ip: IpAddr, destination: SocketAddr) -> Option<IpAddr> {
    // Connecting a UDP socket selects a source address from the local routing
    // table without sending a datagram. The destinations are documentation-only
    // addresses, so this never depends on a live external endpoint.
    UdpSocket::bind(SocketAddr::new(bind_ip, 0))
        .and_then(|socket| {
            socket.connect(destination)?;
            socket.local_addr()
        })
        .ok()
        .map(|address| address.ip())
        .filter(|ip| !ip.is_unspecified())
}

#[derive(Debug, thiserror::Error)]
enum QrError {
    #[error("could not serialize connection payload: {0}")]
    Serialize(serde_json::Error),

    #[error("connection payload does not fit in a QR code: {0}")]
    Encode(qrcode::types::QrError),
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    const AUTH_KEY: &str = "a-secure-test-key-with-at-least-32-bytes";

    #[test]
    fn payload_contains_versioned_url_and_auth_key() {
        let address = "192.0.2.10:8787".parse().unwrap();
        let payload = ConnectionPayload {
            r#type: CONNECTION_TYPE,
            version: CONNECTION_VERSION,
            base_url: base_url(address, false),
            auth_key: AUTH_KEY,
        };
        let payload = serde_json::to_value(payload).unwrap();

        assert_eq!(
            payload,
            serde_json::json!({
                "type": "phi-daemon",
                "version": 1,
                "base_url": "http://192.0.2.10:8787",
                "auth_key": AUTH_KEY,
            })
        );
    }

    #[test]
    fn payload_formats_tls_ipv6_urls_and_renders_a_qr_code() {
        let address = "[2001:db8::1]:9443".parse().unwrap();
        assert_eq!(base_url(address, true), "https://[2001:db8::1]:9443");

        let qr = render(AUTH_KEY, address, true).unwrap();
        assert!(!qr.trim().is_empty());
    }

    #[test]
    fn oversized_keys_fail_qr_rendering_without_leaking_the_key() {
        let auth_key = "x".repeat(4096);
        let address = "127.0.0.1:8787".parse().unwrap();
        let error = render(&auth_key, address, false).unwrap_err().to_string();

        assert!(error.contains("does not fit in a QR code"));
        assert!(!error.contains(&auth_key));
    }

    #[test]
    fn concrete_listener_addresses_are_advertised_unchanged() {
        let address = "192.0.2.20:8080".parse().unwrap();
        assert_eq!(advertised_address(address), address);
    }

    #[test]
    fn wildcard_listener_preserves_family_and_port_but_advertises_a_concrete_ip() {
        let address = "0.0.0.0:8787".parse().unwrap();
        let advertised = advertised_address_with(address, |bind_ip, destination| {
            assert_eq!(bind_ip, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
            assert_eq!(destination, IPV4_ROUTE_PROBE);
            Some(IpAddr::V4(Ipv4Addr::new(192, 168, 9, 138)))
        });

        assert!(advertised.is_ipv4());
        assert!(!advertised.ip().is_unspecified());
        assert_eq!(advertised.port(), address.port());
    }

    #[test]
    fn wildcard_listener_falls_back_to_loopback_when_no_route_is_available() {
        let address = "[::]:8787".parse().unwrap();
        let advertised = advertised_address_with(address, |_, _| None);

        assert_eq!(advertised, "[::1]:8787".parse().unwrap());
    }

    #[test]
    fn wildcard_ipv4_listener_rejects_non_private_route_addresses() {
        let address = "0.0.0.0:8787".parse().unwrap();
        let advertised = advertised_address_with(address, |_, _| {
            Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)))
        });

        assert_eq!(advertised, "127.0.0.1:8787".parse().unwrap());
        assert!(is_private_ipv4(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))));
        assert!(is_private_ipv4(IpAddr::V4(Ipv4Addr::new(172, 16, 2, 3))));
        assert!(is_private_ipv4(IpAddr::V4(Ipv4Addr::new(192, 168, 2, 3))));
    }

    #[test]
    fn serialized_payload_does_not_put_the_key_in_the_url() {
        let payload = ConnectionPayload {
            r#type: CONNECTION_TYPE,
            version: CONNECTION_VERSION,
            base_url: "http://127.0.0.1:8787".to_owned(),
            auth_key: AUTH_KEY,
        };
        let value: Value = serde_json::to_value(payload).unwrap();

        assert!(!value["base_url"].as_str().unwrap().contains(AUTH_KEY));
        assert_eq!(value["auth_key"], AUTH_KEY);
    }
}
