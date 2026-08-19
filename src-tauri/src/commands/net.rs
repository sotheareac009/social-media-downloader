//! Connectivity check for the status indicator.
//!
//! The webview's CSP forbids arbitrary network calls, so the ping is measured
//! here in Rust. We time a plain TCP handshake to a well-known, always-on host
//! rather than an HTTP request: it needs no CSP allowance, no DNS surprises,
//! and the round-trip of the SYN/ACK is a fair proxy for "how's the internet".

use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Result of one connectivity probe.
#[derive(serde::Serialize)]
pub struct NetStatus {
    /// True when at least one probe host answered.
    pub online: bool,
    /// Round-trip of the TCP handshake in milliseconds, when online.
    pub ms: Option<u32>,
    /// Which host answered, for display ("1.1.1.1").
    pub host: Option<String>,
}

/// Hosts we race, in order. All run a TLS listener on 443 and are engineered
/// to be reachable worldwide, so a failure to connect is a real network fault.
const PROBES: &[&str] = &["1.1.1.1:443", "8.8.8.8:443", "9.9.9.9:443"];

const TIMEOUT: Duration = Duration::from_millis(2500);

#[tauri::command]
pub async fn net_ping() -> NetStatus {
    // Blocking sockets on a worker thread keep the async runtime responsive.
    tauri::async_runtime::spawn_blocking(|| {
        for probe in PROBES {
            let Ok(mut addrs) = probe.to_socket_addrs() else {
                continue;
            };
            let Some(addr) = addrs.next() else { continue };
            let start = Instant::now();
            if TcpStream::connect_timeout(&addr, TIMEOUT).is_ok() {
                let ms = start.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
                let host = probe.split(':').next().unwrap_or(probe).to_string();
                return NetStatus {
                    online: true,
                    ms: Some(ms),
                    host: Some(host),
                };
            }
        }
        NetStatus {
            online: false,
            ms: None,
            host: None,
        }
    })
    .await
    .unwrap_or(NetStatus {
        online: false,
        ms: None,
        host: None,
    })
}
