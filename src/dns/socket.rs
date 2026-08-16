//! Socket option tuning for production DNS performance (story 06-001).
//!
//! Sets `SO_REUSEPORT` on UDP/TCP sockets so multiple worker threads can
//! bind the same port and let the kernel distribute incoming packets across
//! them (avoiding the single-socket bottleneck that causes UDP drops under
//! load in hickory-server). Also configures `SO_RCVBUF` / `SO_SNDBUF` to
//! enlarge kernel socket buffers and `SO_KEEPALIVE` on TCP listeners.
//!
//! Because `socket2` is not a direct dependency (it is only transitive via
//! `tokio`), socket options are applied via raw `setsockopt(2)` syscalls on
//! Unix platforms. On non-Unix targets the functions fall back to plain
//! `tokio::net` binds without extra options.
//!
//! ## Platform notes
//!
//! - **Linux**: `SO_REUSEPORT` distributes UDP datagrams across sockets via
//!   a per-socket hash. Requires kernel ≥ 3.9.
//! - **macOS**: `SO_REUSEPORT` is supported (`0x0200`).
//! - Buffer sizes may be silently capped by `net.core.rmem_max` /
//!   `wmem_max` (Linux) or `kern.ipc.maxsockbuf` (macOS). Operators should
//!   raise these sysctls for the configured buffer sizes to take full effect.

use std::io;
use std::net::SocketAddr;
use tokio::net::{TcpListener, UdpSocket};
use tracing::{debug, warn};

/// Socket tuning parameters extracted from [`ServerConfig`](crate::config::ServerConfig).
///
/// This is a lightweight value type passed to [`create_udp_socket`] and
/// [`apply_tcp_options`] so the server module does not need to import the
/// full config struct.
#[derive(Debug, Clone)]
pub struct SocketConfig {
    /// Enable `SO_REUSEPORT` on the socket.
    pub reuse_port: bool,
    /// Receive buffer size in bytes (`SO_RCVBUF`). `0` = leave default.
    pub recv_buffer_size: usize,
    /// Send buffer size in bytes (`SO_SNDBUF`). `0` = leave default.
    pub send_buffer_size: usize,
    /// TCP keepalive idle timeout in seconds. `None` = do not enable keepalive.
    pub tcp_keepalive_secs: Option<u64>,
}

impl SocketConfig {
    /// Build a [`SocketConfig`] for UDP from the server config fields.
    pub fn for_udp(
        reuse_port: bool,
        udp_buffer_size: usize,
    ) -> Self {
        Self {
            reuse_port,
            recv_buffer_size: udp_buffer_size,
            send_buffer_size: udp_buffer_size,
            tcp_keepalive_secs: None,
        }
    }

    /// Build a [`SocketConfig`] for TCP from the server config fields.
    pub fn for_tcp(
        reuse_port: bool,
        tcp_buffer_size: usize,
        tcp_keepalive_secs: Option<u64>,
    ) -> Self {
        Self {
            reuse_port,
            recv_buffer_size: tcp_buffer_size,
            send_buffer_size: tcp_buffer_size,
            tcp_keepalive_secs,
        }
    }
}

// ---------------------------------------------------------------------------
// Unix: raw setsockopt via extern "C"
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod ffi {
    use std::ffi::c_void;
    use std::io;

    extern "C" {
        fn setsockopt(
            socket: i32,
            level: i32,
            name: i32,
            value: *const c_void,
            option_len: u32,
        ) -> i32;
    }

    // SOL_SOCKET and SO_* constants differ between Linux and macOS.
    #[cfg(target_os = "linux")]
    pub(super) const SOL_SOCKET: i32 = 1;
    #[cfg(target_os = "linux")]
    pub(super) const SO_REUSEPORT: i32 = 15;
    #[cfg(target_os = "linux")]
    pub(super) const SO_RCVBUF: i32 = 8;
    #[cfg(target_os = "linux")]
    pub(super) const SO_SNDBUF: i32 = 7;
    #[cfg(target_os = "linux")]
    pub(super) const SO_KEEPALIVE: i32 = 9;
    #[cfg(target_os = "linux")]
    pub(super) const IPPROTO_TCP: i32 = 6;
    #[cfg(target_os = "linux")]
    pub(super) const TCP_KEEPIDLE: i32 = 4;

    #[cfg(target_os = "macos")]
    pub(super) const SOL_SOCKET: i32 = 0xffff;
    #[cfg(target_os = "macos")]
    pub(super) const SO_REUSEPORT: i32 = 0x0200;
    #[cfg(target_os = "macos")]
    pub(super) const SO_RCVBUF: i32 = 0x1002;
    #[cfg(target_os = "macos")]
    pub(super) const SO_SNDBUF: i32 = 0x1001;
    #[cfg(target_os = "macos")]
    pub(super) const SO_KEEPALIVE: i32 = 0x0008;
    #[cfg(target_os = "macos")]
    pub(super) const IPPROTO_TCP: i32 = 6;
    #[cfg(target_os = "macos")]
    pub(super) const TCP_KEEPALIVE: i32 = 0x10;

    // Fallback for other Unix targets (FreeBSD, etc.) — use the Linux
    // values which are common across many BSD derivatives for these
    // well-known constants. SO_REUSEPORT may not exist on very old kernels;
    // the caller logs and continues on error.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) const SOL_SOCKET: i32 = 0xffff;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) const SO_REUSEPORT: i32 = 0x0200;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) const SO_RCVBUF: i32 = 0x1002;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) const SO_SNDBUF: i32 = 0x1001;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) const SO_KEEPALIVE: i32 = 0x0008;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) const IPPROTO_TCP: i32 = 6;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) const TCP_KEEPIDLE: i32 = 4;

    /// Call `setsockopt` with an `i32` value. Returns `io::Result<()>`.
    pub(super) fn setsockopt_i32(fd: i32, level: i32, name: i32, val: i32) -> io::Result<()> {
        let val: i32 = val;
        let ret = unsafe {
            setsockopt(
                fd,
                level,
                name,
                &val as *const i32 as *const c_void,
                std::mem::size_of::<i32>() as u32,
            )
        };
        if ret == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

/// Create a bound, non-blocking `tokio::net::UdpSocket` with `SO_REUSEPORT`
/// and buffer-size socket options applied per `config`.
///
/// On Unix the socket is created via `std::net::UdpSocket::bind`, options
/// are set via raw `setsockopt`, the socket is switched to non-blocking
/// mode, and then converted to a `tokio::net::UdpSocket`. On non-Unix
/// platforms this falls back to `UdpSocket::bind` with no extra options.
pub async fn create_udp_socket(addr: &str, config: &SocketConfig) -> io::Result<UdpSocket> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;

        let std_sock = std::net::UdpSocket::bind(addr)?;
        let fd = std_sock.as_raw_fd();

        if config.reuse_port {
            if let Err(e) = ffi::setsockopt_i32(fd, ffi::SOL_SOCKET, ffi::SO_REUSEPORT, 1) {
                warn!(error = %e, "SO_REUSEPORT not set on UDP socket (platform may not support it)");
            } else {
                debug!("SO_REUSEPORT enabled on UDP socket");
            }
        }
        if config.recv_buffer_size > 0 {
            if let Err(e) =
                ffi::setsockopt_i32(fd, ffi::SOL_SOCKET, ffi::SO_RCVBUF, config.recv_buffer_size as i32)
            {
                warn!(error = %e, size = config.recv_buffer_size, "SO_RCVBUF not set on UDP socket");
            }
        }
        if config.send_buffer_size > 0 {
            if let Err(e) =
                ffi::setsockopt_i32(fd, ffi::SOL_SOCKET, ffi::SO_SNDBUF, config.send_buffer_size as i32)
            {
                warn!(error = %e, size = config.send_buffer_size, "SO_SNDBUF not set on UDP socket");
            }
        }

        std_sock.set_nonblocking(true)?;
        UdpSocket::from_std(std_sock)
    }

    #[cfg(not(unix))]
    {
        let _ = config;
        UdpSocket::bind(addr).await
    }
}

/// Create a bound, non-blocking `tokio::net::TcpListener` with `SO_REUSEPORT`
/// and buffer-size socket options applied per `config`.
///
/// On Unix the listener is created via `std::net::TcpListener::bind`,
/// options are set via raw `setsockopt`, the socket is switched to
/// non-blocking mode, and then converted to a `tokio::net::TcpListener`.
pub async fn create_tcp_listener(addr: &str, config: &SocketConfig) -> io::Result<TcpListener> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;

        let std_listener = std::net::TcpListener::bind(addr)?;
        let fd = std_listener.as_raw_fd();

        if config.reuse_port {
            if let Err(e) = ffi::setsockopt_i32(fd, ffi::SOL_SOCKET, ffi::SO_REUSEPORT, 1) {
                warn!(error = %e, "SO_REUSEPORT not set on TCP listener (platform may not support it)");
            } else {
                debug!("SO_REUSEPORT enabled on TCP listener");
            }
        }
        if config.recv_buffer_size > 0 {
            if let Err(e) =
                ffi::setsockopt_i32(fd, ffi::SOL_SOCKET, ffi::SO_RCVBUF, config.recv_buffer_size as i32)
            {
                warn!(error = %e, size = config.recv_buffer_size, "SO_RCVBUF not set on TCP listener");
            }
        }
        if config.send_buffer_size > 0 {
            if let Err(e) =
                ffi::setsockopt_i32(fd, ffi::SOL_SOCKET, ffi::SO_SNDBUF, config.send_buffer_size as i32)
            {
                warn!(error = %e, size = config.send_buffer_size, "SO_SNDBUF not set on TCP listener");
            }
        }
        if let Some(secs) = config.tcp_keepalive_secs {
            if let Err(e) = ffi::setsockopt_i32(fd, ffi::SOL_SOCKET, ffi::SO_KEEPALIVE, 1) {
                warn!(error = %e, "SO_KEEPALIVE not set on TCP listener");
            } else {
                debug!(secs, "SO_KEEPALIVE enabled on TCP listener");
            }
            // Set the keepalive idle time. The option name differs:
            // TCP_KEEPIDLE (Linux) vs TCP_KEEPALIVE (macOS).
            #[cfg(target_os = "linux")]
            {
                if let Err(e) =
                    ffi::setsockopt_i32(fd, ffi::IPPROTO_TCP, ffi::TCP_KEEPIDLE, secs as i32)
                {
                    warn!(error = %e, "TCP_KEEPIDLE not set on TCP listener");
                }
            }
            #[cfg(target_os = "macos")]
            {
                if let Err(e) =
                    ffi::setsockopt_i32(fd, ffi::IPPROTO_TCP, ffi::TCP_KEEPALIVE, secs as i32)
                {
                    warn!(error = %e, "TCP_KEEPALIVE not set on TCP listener");
                }
            }
        }

        std_listener.set_nonblocking(true)?;
        TcpListener::from_std(std_listener)
    }

    #[cfg(not(unix))]
    {
        let _ = config;
        TcpListener::bind(addr).await
    }
}

/// Convenience: bind a UDP socket and return the local address, logging
/// the effective configuration.
pub async fn bind_udp(addr: &str, config: &SocketConfig) -> io::Result<(UdpSocket, SocketAddr)> {
    let socket = create_udp_socket(addr, config).await?;
    let bound = socket.local_addr()?;
    tracing::info!(
        addr = %bound,
        reuse_port = config.reuse_port,
        recv_buffer = config.recv_buffer_size,
        send_buffer = config.send_buffer_size,
        "UDP socket bound with performance tuning"
    );
    Ok((socket, bound))
}

/// Convenience: bind a TCP listener and return the local address, logging
/// the effective configuration.
pub async fn bind_tcp(addr: &str, config: &SocketConfig) -> io::Result<(TcpListener, SocketAddr)> {
    let listener = create_tcp_listener(addr, config).await?;
    let bound = listener.local_addr()?;
    tracing::info!(
        addr = %bound,
        reuse_port = config.reuse_port,
        recv_buffer = config.recv_buffer_size,
        send_buffer = config.send_buffer_size,
        keepalive_secs = ?config.tcp_keepalive_secs,
        "TCP listener bound with performance tuning"
    );
    Ok((listener, bound))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_udp_socket_with_reuseport() {
        let config = SocketConfig::for_udp(true, 1_048_576);
        // Bind to an ephemeral port on localhost.
        let socket = create_udp_socket("127.0.0.1:0", &config).await;
        assert!(socket.is_ok(), "UDP socket creation should succeed");
        let socket = socket.unwrap();
        let addr = socket.local_addr().unwrap();
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
    }

    #[tokio::test]
    async fn create_udp_socket_no_reuseport() {
        let config = SocketConfig::for_udp(false, 0);
        let socket = create_udp_socket("127.0.0.1:0", &config).await;
        assert!(socket.is_ok(), "UDP socket creation should succeed without reuseport");
    }

    #[tokio::test]
    async fn create_tcp_listener_with_options() {
        let config = SocketConfig::for_tcp(true, 65_536, Some(30));
        let listener = create_tcp_listener("127.0.0.1:0", &config).await;
        assert!(listener.is_ok(), "TCP listener creation should succeed");
        let listener = listener.unwrap();
        let addr = listener.local_addr().unwrap();
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
    }

    #[tokio::test]
    async fn create_tcp_listener_no_keepalive() {
        let config = SocketConfig::for_tcp(false, 0, None);
        let listener = create_tcp_listener("127.0.0.1:0", &config).await;
        assert!(listener.is_ok(), "TCP listener creation should succeed without keepalive");
    }

    #[tokio::test]
    async fn bind_udp_returns_addr() {
        let config = SocketConfig::for_udp(true, 65_536);
        let result = bind_udp("127.0.0.1:0", &config).await;
        assert!(result.is_ok());
        let (_socket, addr) = result.unwrap();
        assert!(addr.port() > 0);
    }

    #[tokio::test]
    async fn bind_tcp_returns_addr() {
        let config = SocketConfig::for_tcp(true, 65_536, None);
        let result = bind_tcp("127.0.0.1:0", &config).await;
        assert!(result.is_ok());
        let (_listener, addr) = result.unwrap();
        assert!(addr.port() > 0);
    }

    #[test]
    fn socket_config_for_udp_sets_both_buffers() {
        let config = SocketConfig::for_udp(true, 4_194_304);
        assert!(config.reuse_port);
        assert_eq!(config.recv_buffer_size, 4_194_304);
        assert_eq!(config.send_buffer_size, 4_194_304);
        assert!(config.tcp_keepalive_secs.is_none());
    }

    #[test]
    fn socket_config_for_tcp_sets_keepalive() {
        let config = SocketConfig::for_tcp(false, 262_144, Some(60));
        assert!(!config.reuse_port);
        assert_eq!(config.recv_buffer_size, 262_144);
        assert_eq!(config.send_buffer_size, 262_144);
        assert_eq!(config.tcp_keepalive_secs, Some(60));
    }
}
