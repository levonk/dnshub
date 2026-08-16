//! Read-only TFTP server (RFC 1350).
//!
//! The server binds a UDP socket (default port 69) and services RRQ
//! (read) requests only. WRQ is rejected with an access-violation
//! error. Files are served from a configured `root_dir` and must match
//! the configured `allowlist` (glob patterns). Path traversal (`..`,
//! absolute paths) is rejected.
//!
//! Each transfer uses a fresh ephemeral socket (TID) per RFC 1350: the
//! server replies from a new port so it can multiplex concurrent
//! transfers. Blocksize negotiation (RFC 2348) is out of scope —
//! transfers use the fixed 512-byte block size.

pub mod protocol;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::net::UdpSocket;
use tracing::{debug, warn};

pub use protocol::{
    error_code, mode, opcode, DecodeError, Packet, BLOCK_SIZE,
};

use crate::dhcp::pxe::config::TftpConfig;

/// A running TFTP server handle. Spawned via [`TftpServer::start`].
pub struct TftpServer {
    config: Arc<TftpConfig>,
}

impl TftpServer {
    /// Construct a server from config. Does not bind until [`start`](Self::start).
    pub fn new(config: TftpConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    /// Bind the listening socket and spawn the receive loop.
    ///
    /// Returns immediately; the server runs as a background tokio task.
    /// The task is cancelled when the returned [`tokio::task::JoinHandle`]
    /// is dropped or awaited to completion.
    pub async fn start(self) -> std::io::Result<tokio::task::JoinHandle<()>> {
        if !self.config.enabled {
            debug!("tftp server disabled, not starting");
            // Return a no-op handle that completes immediately.
            return Ok(tokio::spawn(async {}));
        }
        let listen = if self.config.listen.is_empty() {
            "0.0.0.0:69"
        } else {
            self.config.listen.as_str()
        };
        let sock = UdpSocket::bind(listen).await?;
        debug!(addr = %sock.local_addr()?, "tftp server listening");
        let config = self.config.clone();
        let handle = tokio::spawn(async move {
            Self::serve(sock, config).await;
        });
        Ok(handle)
    }

    async fn serve(sock: UdpSocket, config: Arc<TftpConfig>) {
        let mut buf = vec![0u8; 65535];
        loop {
            match sock.recv_from(&mut buf).await {
                Ok((n, peer)) => {
                    let pkt = match Packet::decode(&buf[..n]) {
                        Ok(p) => p,
                        Err(e) => {
                            warn!(%peer, error = %e, "malformed tftp packet, ignoring");
                            continue;
                        }
                    };
                    let config = config.clone();
                    // Each transfer gets its own ephemeral socket (TID).
                    tokio::spawn(async move {
                        if let Err(e) = handle_request(pkt, peer, config).await {
                            warn!(%peer, error = %e, "tftp transfer failed");
                        }
                    });
                }
                Err(e) => {
                    warn!(error = %e, "tftp recv_from error");
                    continue;
                }
            }
        }
    }
}

/// Handle a single client request on a fresh ephemeral socket.
async fn handle_request(
    pkt: Packet,
    peer: SocketAddr,
    config: Arc<TftpConfig>,
) -> std::io::Result<()> {
    let transfer_sock = UdpSocket::bind("0.0.0.0:0").await?;
    transfer_sock.connect(peer).await?;

    match pkt {
        Packet::Rrq {
            filename,
            mode,
            options: _,
        } => {
            if mode.to_ascii_lowercase() != mode::OCTET {
                send_error(&transfer_sock, error_code::ILLEGAL_OPERATION, "only octet mode supported").await?;
                return Ok(());
            }
            match resolve_file(&config.root_dir, &filename, &config.allowlist) {
                Ok(path) => {
                    debug!(%peer, file = %path.display(), "tftp rrq serving");
                    serve_file(&transfer_sock, &path).await?;
                }
                Err(ResolveError::NotFound) => {
                    send_error(&transfer_sock, error_code::FILE_NOT_FOUND, "file not found").await?;
                }
                Err(ResolveError::AccessViolation) => {
                    send_error(&transfer_sock, error_code::ACCESS_VIOLATION, "access violation").await?;
                }
            }
        }
        Packet::Wrq { .. } => {
            send_error(&transfer_sock, error_code::ACCESS_VIOLATION, "write not supported").await?;
        }
        // A server-only listener does not expect DATA/ACK/OACK as the
        // first packet on a fresh TID; ignore them.
        _ => {
            debug!(%peer, "unexpected initial tftp packet, ignoring");
        }
    }
    Ok(())
}

/// Resolve a requested filename against the root directory and allowlist.
fn resolve_file(
    root_dir: &str,
    filename: &str,
    allowlist: &[String],
) -> Result<PathBuf, ResolveError> {
    // Reject path traversal and absolute paths.
    if filename.is_empty()
        || filename.contains("..")
        || filename.contains('\\')
        || filename.starts_with('/')
    {
        return Err(ResolveError::AccessViolation);
    }
    let root = Path::new(root_dir);
    let candidate = root.join(filename);

    // Canonicalize to ensure the resolved path stays under root.
    // If root doesn't exist, treat as not found.
    let canon_root = root.canonicalize().map_err(|_| ResolveError::NotFound)?;
    let canon_candidate = match candidate.canonicalize() {
        Ok(p) => p,
        Err(_) => return Err(ResolveError::NotFound),
    };
    if !canon_candidate.starts_with(&canon_root) {
        return Err(ResolveError::AccessViolation);
    }
    if !canon_candidate.is_file() {
        return Err(ResolveError::NotFound);
    }

    // Enforce allowlist (glob match on the basename).
    if !allowlist.is_empty() {
        let basename = canon_candidate
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let allowed = allowlist
            .iter()
            .any(|pat| glob_matches(pat, basename));
        if !allowed {
            return Err(ResolveError::AccessViolation);
        }
    }

    Ok(canon_candidate)
}

#[derive(Debug)]
enum ResolveError {
    NotFound,
    AccessViolation,
}

/// Serve a file: send DATA blocks (512 bytes each), waiting for an ACK
/// after every block. A block smaller than 512 bytes marks EOF.
async fn serve_file(sock: &UdpSocket, path: &Path) -> std::io::Result<()> {
    let data = tokio::fs::read(path).await?;
    let mut block: u16 = 1;
    let mut offset = 0usize;
    loop {
        let end = (offset + BLOCK_SIZE).min(data.len());
        let chunk = &data[offset..end];
        let is_final = chunk.len() < BLOCK_SIZE;

        let pkt = Packet::Data {
            block,
            data: chunk.to_vec(),
        };
        sock.send(&pkt.encode()).await?;

        // Wait for ACK of this block.
        let mut ack_buf = vec![0u8; 4];
        match sock.recv(&mut ack_buf).await {
            Ok(n) => match Packet::decode(&ack_buf[..n]) {
                Ok(Packet::Ack { block: ack_block }) if ack_block == block => {
                    if is_final {
                        return Ok(());
                    }
                    offset = end;
                    block = block.wrapping_add(1);
                }
                Ok(Packet::Error { .. }) => {
                    return Ok(()); // client aborted
                }
                _ => {
                    // Unexpected packet; resend current block (simplified
                    // retransmission — no retry limit for this story).
                    continue;
                }
            },
            Err(e) => return Err(e),
        }
    }
}

/// Send a TFTP ERROR packet.
async fn send_error(sock: &UdpSocket, code: u16, msg: &str) -> std::io::Result<()> {
    let pkt = Packet::Error {
        code,
        msg: msg.to_string(),
    };
    sock.send(&pkt.encode()).await?;
    Ok(())
}

/// Minimal glob matcher supporting `*` (any sequence) and `?` (single
/// char). Used for the TFTP allowlist patterns like `*.efi`.
fn glob_matches(pattern: &str, name: &str) -> bool {
    glob_match(pattern.as_bytes(), name.as_bytes())
}

fn glob_match(pat: &[u8], name: &[u8]) -> bool {
    // Iterative DP-style match over (pat_idx, name_idx).
    let (m, n) = (pat.len(), name.len());
    let mut dp = vec![vec![false; n + 1]; m + 1];
    dp[0][0] = true;
    for i in 1..=m {
        if pat[i - 1] == b'*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=m {
        for j in 1..=n {
            match pat[i - 1] {
                b'*' => dp[i][j] = dp[i - 1][j] || dp[i][j - 1],
                b'?' => dp[i][j] = dp[i - 1][j - 1],
                c => dp[i][j] = dp[i - 1][j - 1] && name[j - 1] == c,
            }
        }
    }
    dp[m][n]
}

/// A scoped temporary directory cleaned up on drop (avoids pulling in the
/// `tempdir`/`tempfile` crate, which is not in `Cargo.toml`).
#[cfg(test)]
struct TempDir {
    path: PathBuf,
}

#[cfg(test)]
impl TempDir {
    fn new(prefix: &str) -> std::io::Result<Self> {
        let mut path = std::env::temp_dir();
        let unique = format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        path.push(unique);
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Helper: write a temp file and return its (root_dir, full_path, basename).
    fn make_temp_file(name: &str, contents: &[u8]) -> (TempDir, PathBuf) {
        let dir = TempDir::new("dnshub-tftp-test").unwrap();
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents).unwrap();
        (dir, path)
    }

    #[test]
    fn glob_matches_star_efi() {
        assert!(glob_matches("*.efi", "grubx64.efi"));
        assert!(glob_matches("*.efi", "grubaa64.efi"));
        assert!(!glob_matches("*.efi", "pxelinux.0"));
    }

    #[test]
    fn glob_matches_exact_name() {
        assert!(glob_matches("pxelinux.0", "pxelinux.0"));
        assert!(!glob_matches("pxelinux.0", "grubx64.efi"));
    }

    #[test]
    fn glob_matches_question_mark() {
        assert!(glob_matches("boot?.ipxe", "boot1.ipxe"));
        assert!(glob_matches("boot?.ipxe", "bootA.ipxe"));
        assert!(!glob_matches("boot?.ipxe", "boot12.ipxe"));
    }

    #[test]
    fn glob_matches_star_only() {
        assert!(glob_matches("*", "anything.efi"));
        assert!(glob_matches("*", "no_ext"));
    }

    #[test]
    fn resolve_file_serves_allowed_file() {
        let (dir, path) = make_temp_file("grubx64.efi", b"efi-bytes");
        let root = dir.path().to_str().unwrap();
        let allowlist = vec!["*.efi".to_string()];
        let resolved = resolve_file(root, "grubx64.efi", &allowlist).unwrap();
        // canonicalize the expected path too (macOS resolves /var → /private/var).
        assert_eq!(resolved, path.canonicalize().unwrap());
    }

    #[test]
    fn resolve_file_rejects_not_in_allowlist() {
        let (dir, _path) = make_temp_file("README.md", b"docs");
        let root = dir.path().to_str().unwrap();
        let allowlist = vec!["*.efi".to_string()];
        let err = resolve_file(root, "README.md", &allowlist).unwrap_err();
        assert!(matches!(err, ResolveError::AccessViolation));
    }

    #[test]
    fn resolve_file_empty_allowlist_allows_any() {
        let (dir, path) = make_temp_file("anything.txt", b"hi");
        let root = dir.path().to_str().unwrap();
        let resolved = resolve_file(root, "anything.txt", &[]).unwrap();
        assert_eq!(resolved, path.canonicalize().unwrap());
    }

    #[test]
    fn resolve_file_rejects_path_traversal() {
        let (dir, _path) = make_temp_file("grubx64.efi", b"x");
        let root = dir.path().to_str().unwrap();
        let err = resolve_file(root, "../etc/passwd", &[]).unwrap_err();
        assert!(matches!(err, ResolveError::AccessViolation));
    }

    #[test]
    fn resolve_file_rejects_absolute_path() {
        let (dir, _path) = make_temp_file("grubx64.efi", b"x");
        let root = dir.path().to_str().unwrap();
        let err = resolve_file(root, "/etc/passwd", &[]).unwrap_err();
        assert!(matches!(err, ResolveError::AccessViolation));
    }

    #[test]
    fn resolve_file_rejects_backslash_path() {
        let (dir, _path) = make_temp_file("grubx64.efi", b"x");
        let root = dir.path().to_str().unwrap();
        let err = resolve_file(root, "sub\\..\\grubx64.efi", &[]).unwrap_err();
        assert!(matches!(err, ResolveError::AccessViolation));
    }

    #[test]
    fn resolve_file_not_found() {
        let (dir, _path) = make_temp_file("grubx64.efi", b"x");
        let root = dir.path().to_str().unwrap();
        let err = resolve_file(root, "missing.efi", &[]).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound));
    }

    #[test]
    fn resolve_file_rejects_empty_filename() {
        let (dir, _path) = make_temp_file("grubx64.efi", b"x");
        let root = dir.path().to_str().unwrap();
        let err = resolve_file(root, "", &[]).unwrap_err();
        assert!(matches!(err, ResolveError::AccessViolation));
    }

    #[tokio::test]
    async fn serve_file_sends_blocks_and_final_short_block() {
        // Build a payload larger than one block to exercise block numbering.
        let payload: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
        let (_dir, path) = make_temp_file("data.bin", &payload);

        // Create a connected UDP pair to act as the "client".
        let server_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server_sock.local_addr().unwrap();
        let client_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client_sock.connect(server_addr).await.unwrap();
        server_sock
            .connect(client_sock.local_addr().unwrap())
            .await
            .unwrap();

        let path_clone = path.clone();
        let serve_task = tokio::spawn(async move {
            serve_file(&server_sock, &path_clone).await
        });

        // Client: ACK each DATA block and collect the bytes.
        let mut received: Vec<u8> = Vec::new();
        let mut expected_block: u16 = 1;
        let mut buf = vec![0u8; 600];
        loop {
            let n = client_sock.recv(&mut buf).await.unwrap();
            let pkt = Packet::decode(&buf[..n]).unwrap();
            match pkt {
                Packet::Data { block, data } => {
                    assert_eq!(block, expected_block);
                    received.extend_from_slice(&data);
                    let ack = Packet::Ack { block };
                    client_sock.send(&ack.encode()).await.unwrap();
                    expected_block = expected_block.wrapping_add(1);
                    if data.len() < BLOCK_SIZE {
                        break;
                    }
                }
                other => panic!("expected DATA, got {other:?}"),
            }
        }
        serve_task.await.unwrap().unwrap();
        assert_eq!(received, payload);

    }

    #[tokio::test]
    async fn serve_file_single_short_block() {
        let payload = b"hello".to_vec();
        let (_dir, path) = make_temp_file("small.txt", &payload);

        let server_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server_sock.local_addr().unwrap();
        let client_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client_sock.connect(server_addr).await.unwrap();
        server_sock
            .connect(client_sock.local_addr().unwrap())
            .await
            .unwrap();

        let path_clone = path.clone();
        let serve_task = tokio::spawn(async move {
            serve_file(&server_sock, &path_clone).await
        });

        let mut buf = vec![0u8; 600];
        let n = client_sock.recv(&mut buf).await.unwrap();
        let pkt = Packet::decode(&buf[..n]).unwrap();
        match pkt {
            Packet::Data { block, data } => {
                assert_eq!(block, 1);
                assert_eq!(data, b"hello");
                assert!(data.len() < BLOCK_SIZE);
            }
            other => panic!("expected DATA, got {other:?}"),
        }
        let ack = Packet::Ack { block: 1 };
        client_sock.send(&ack.encode()).await.unwrap();
        serve_task.await.unwrap().unwrap();

    }

    #[tokio::test]
    async fn handle_request_rejects_wrq_with_access_violation() {
        let (dir, _path) = make_temp_file("target.txt", b"x");
        let config = Arc::new(TftpConfig {
            enabled: true,
            listen: "0.0.0.0:0".to_string(),
            root_dir: dir.path().to_str().unwrap().to_string(),
            allowlist: vec![],
        });

        // Unconnected client socket so it can receive the ERROR the
        // transfer socket (a fresh TID) sends back.
        let client_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer = client_sock.local_addr().unwrap();

        let wrq = Packet::Wrq {
            filename: "target.txt".to_string(),
            mode: mode::OCTET.to_string(),
            options: vec![],
        };
        let cfg = config.clone();
        let handle = tokio::spawn(async move {
            handle_request(wrq, peer, cfg).await
        });

        let mut buf = vec![0u8; 600];
        let (n, _from) = client_sock.recv_from(&mut buf).await.unwrap();
        let pkt = Packet::decode(&buf[..n]).unwrap();
        match pkt {
            Packet::Error { code, msg } => {
                assert_eq!(code, error_code::ACCESS_VIOLATION);
                assert!(msg.contains("write"));
            }
            other => panic!("expected ERROR, got {other:?}"),
        }
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn handle_request_rrq_serves_file() {
        let payload = b"netboot-payload".to_vec();
        let (dir, _path) = make_temp_file("grubx64.efi", &payload);
        let config = Arc::new(TftpConfig {
            enabled: true,
            listen: "0.0.0.0:0".to_string(),
            root_dir: dir.path().to_str().unwrap().to_string(),
            allowlist: vec!["*.efi".to_string()],
        });

        // Unconnected client so it can recv_from the transfer socket's
        // ephemeral TID and reply to it directly.
        let client_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer = client_sock.local_addr().unwrap();

        let rrq = Packet::Rrq {
            filename: "grubx64.efi".to_string(),
            mode: mode::OCTET.to_string(),
            options: vec![],
        };
        let cfg = config.clone();
        let handle = tokio::spawn(async move {
            handle_request(rrq, peer, cfg).await
        });

        let mut received: Vec<u8> = Vec::new();
        let mut expected_block: u16 = 1;
        let mut buf = vec![0u8; 600];
        loop {
            let (n, transfer_tid) = client_sock.recv_from(&mut buf).await.unwrap();
            let pkt = Packet::decode(&buf[..n]).unwrap();
            match pkt {
                Packet::Data { block, data } => {
                    assert_eq!(block, expected_block);
                    received.extend_from_slice(&data);
                    let ack = Packet::Ack { block };
                    client_sock.send_to(&ack.encode(), transfer_tid).await.unwrap();
                    expected_block = expected_block.wrapping_add(1);
                    if data.len() < BLOCK_SIZE {
                        break;
                    }
                }
                other => panic!("expected DATA, got {other:?}"),
            }
        }
        handle.await.unwrap().unwrap();
        assert_eq!(received, payload);
    }
}
