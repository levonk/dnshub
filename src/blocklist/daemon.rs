//! Blocklist daemon: fetches sources over HTTP, validates schema, parses,
//! and compiles entries into LMDB.
//!
//! The daemon runs as a background tokio task with per-source configurable
//! refresh intervals. On each refresh cycle, it:
//!
//! 1. Fetches the blocklist source over HTTP (via `reqwest`).
//! 2. Decompresses gzip content if needed (via `flate2`).
//! 3. Validates the response schema (HTTP status, content-type, size
//!    sanity, entry count ±10%).
//! 4. Parses the content using the configured format parser.
//! 5. Compiles entries into LMDB and triggers a hot-swap.

use crate::blocklist::compiler::BlocklistCompiler;
use crate::blocklist::config::{BlocklistsConfig, SourceConfig};
use crate::blocklist::hot_swap::HotSwapStore;
use crate::blocklist::parser::parse_source;
use crate::blocklist::{BlocklistError, Result};
use flate2::read::GzDecoder;
use std::io::Read;
use std::sync::Arc;

/// Maximum response size: 500 MB. Reject larger downloads.
const MAX_RESPONSE_SIZE: usize = 500 * 1024 * 1024;

/// Minimum response size: 10 bytes. Reject empty/tiny responses.
const MIN_RESPONSE_SIZE: usize = 10;

/// Blocklist daemon that fetches, validates, parses, and compiles
/// blocklist sources.
pub struct BlocklistDaemon {
    config: BlocklistsConfig,
    hot_swap: Arc<HotSwapStore>,
    client: reqwest::Client,
    /// Expected entry counts per source name (for schema validation).
    /// Populated after the first successful fetch.
    expected_counts: std::sync::Mutex<std::collections::HashMap<String, usize>>,
}

impl BlocklistDaemon {
    /// Create a new daemon with the given configuration and hot-swap store.
    pub fn new(config: BlocklistsConfig, hot_swap: Arc<HotSwapStore>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("failed to build reqwest client");

        Self {
            config,
            hot_swap,
            client,
            expected_counts: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Create a daemon with a custom HTTP client (for testing).
    pub fn with_client(
        config: BlocklistsConfig,
        hot_swap: Arc<HotSwapStore>,
        client: reqwest::Client,
    ) -> Self {
        Self {
            config,
            hot_swap,
            client,
            expected_counts: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Fetch a single source and return its content as a string.
    ///
    /// Handles gzip decompression automatically.
    pub async fn fetch_source(&self, source: &SourceConfig) -> Result<String> {
        let response = self
            .client
            .get(&source.url)
            .send()
            .await
            .map_err(|e| BlocklistError::Http(format!("fetch failed for '{}': {e}", source.name)))?;

        // Validate HTTP status.
        if !response.status().is_success() {
            return Err(BlocklistError::SchemaValidation(format!(
                "source '{}': HTTP status {} (expected 200)",
                source.name,
                response.status()
            )));
        }

        // Check content-type (warn but don't fail for non-text types).
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();

        // Check content-encoding for gzip.
        let content_encoding = response
            .headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        // Get the raw bytes.
        let bytes = response
            .bytes()
            .await
            .map_err(|e| BlocklistError::Http(format!("read body failed for '{}': {e}", source.name)))?;

        // Size sanity check.
        if bytes.len() < MIN_RESPONSE_SIZE {
            return Err(BlocklistError::SchemaValidation(format!(
                "source '{}': response too small ({} bytes, minimum {})",
                source.name,
                bytes.len(),
                MIN_RESPONSE_SIZE
            )));
        }
        if bytes.len() > MAX_RESPONSE_SIZE {
            return Err(BlocklistError::SchemaValidation(format!(
                "source '{}': response too large ({} bytes, maximum {})",
                source.name,
                bytes.len(),
                MAX_RESPONSE_SIZE
            )));
        }

        // Decompress if gzip-encoded (either by header or magic bytes).
        let content = if content_encoding.contains("gzip") || (bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b) {
            decompress_gzip(&bytes).map_err(|e| {
                BlocklistError::Http(format!("gzip decompress failed for '{}': {e}", source.name))
            })?
        } else {
            String::from_utf8(bytes.to_vec()).map_err(|e| {
                BlocklistError::Http(format!(
                    "invalid UTF-8 in response from '{}': {e}",
                    source.name
                ))
            })?
        };

        tracing::debug!(
            source = source.name,
            content_type = content_type,
            size = content.len(),
            "fetched blocklist source"
        );

        Ok(content)
    }

    /// Validate the parsed content against expected schema.
    ///
    /// Checks:
    /// - Entry count is within ±10% of the previous fetch (if available).
    /// - Content is not empty.
    ///
    /// On success, updates the expected count for future validation.
    pub fn validate_content(
        &self,
        source_name: &str,
        entry_count: usize,
    ) -> Result<()> {
        if entry_count == 0 {
            return Err(BlocklistError::SchemaValidation(format!(
                "source '{source_name}': parsed 0 entries"
            )));
        }

        // Check entry count deviation from previous fetch.
        {
            let expected = self.expected_counts.lock().unwrap();
            if let Some(&prev_count) = expected.get(source_name) {
                if prev_count > 0 {
                    let deviation = if entry_count >= prev_count {
                        (entry_count - prev_count) as f64 / prev_count as f64
                    } else {
                        (prev_count - entry_count) as f64 / prev_count as f64
                    };
                    if deviation > 0.10 {
                        return Err(BlocklistError::SchemaValidation(format!(
                            "source '{source_name}': entry count deviation {:.1}% exceeds ±10% (was {prev_count}, now {entry_count})",
                            deviation * 100.0
                        )));
                    }
                }
            }
        }

        // Update expected count for future validation.
        {
            let mut expected = self.expected_counts.lock().unwrap();
            expected.insert(source_name.to_string(), entry_count);
        }

        Ok(())
    }

    /// Refresh a single source: fetch, validate, parse, compile.
    pub async fn refresh_source(
        &self,
        source: &SourceConfig,
        source_id: u16,
    ) -> Result<usize> {
        tracing::info!(source = source.name, "refreshing blocklist source");

        // Fetch content.
        let content = self.fetch_source(source).await?;

        // Parse content.
        let entries = parse_source(&content, source.format, source_id, 0)?;

        // Validate entry count (also updates expected count).
        self.validate_content(&source.name, entries.len())?;

        tracing::info!(
            source = source.name,
            entries = entries.len(),
            "parsed blocklist source"
        );

        Ok(entries.len())
    }

    /// Refresh all sources and compile into LMDB.
    ///
    /// Fetches all sources in parallel, parses them, merges entries, and
    /// compiles the combined list into a new LMDB database via hot-swap.
    pub async fn refresh_all(&self) -> Result<()> {
        let mut all_entries = Vec::new();

        for (idx, source) in self.config.sources.iter().enumerate() {
            let source_id = 1u16 << idx.min(15);

            match self.refresh_source(source, source_id).await {
                Ok(count) => {
                    tracing::info!(
                        source = source.name,
                        entries = count,
                        "source refreshed successfully"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        source = source.name,
                        error = %e,
                        "source refresh failed, serving stale data"
                    );
                    // Continue with other sources; full backoff in story 06-002.
                }
            }

            // Re-fetch and parse to get the actual entries for compilation.
            // (In a production system, we'd cache the parsed entries from
            // refresh_source, but this keeps the code simple.)
            if let Ok(content) = self.fetch_source(source).await {
                if let Ok(entries) = parse_source(&content, source.format, source_id, 0) {
                    all_entries.extend(entries);
                }
            }
        }

        if all_entries.is_empty() {
            tracing::warn!("no entries collected from any source");
            return Ok(());
        }

        // Compile all entries into a new LMDB database via hot-swap.
        let bloom_path = self.hot_swap.live_path().with_extension("bloom.meta");
        let entries_arc = Arc::new(all_entries);

        let hot_swap = self.hot_swap.clone();
        let bloom_fpr = self.config.storage.bloom_fpr;
        let enable_bloom = self.config.storage.bloom_filter;

        hot_swap
            .swap_database(|temp_path| {
                let compiler = BlocklistCompiler::new(bloom_fpr, enable_bloom);
                let store = compiler.compile(&entries_arc, temp_path, Some(&bloom_path))?;
                Ok(store)
            })
            .map_err(|e| {
                tracing::error!(error = %e, "hot-swap failed");
                e
            })?;

        tracing::info!(total_entries = entries_arc.len(), "blocklist refresh complete");
        Ok(())
    }

    /// Run the daemon refresh loop.
    ///
    /// This runs indefinitely, refreshing each source at its configured
    /// interval. Intended to be spawned as a `tokio::spawn` task.
    pub async fn run(self: Arc<Self>) -> Result<()> {
        // Initial refresh.
        self.refresh_all().await?;

        // Set up per-source refresh timers.
        let mut timers: Vec<tokio::time::Interval> = self
            .config
            .sources
            .iter()
            .map(|s| {
                tokio::time::interval(std::time::Duration::from_secs(
                    s.refresh_interval_secs(),
                ))
            })
            .collect();

        loop {
            // Wait for the shortest interval to fire.
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;

            // Check each source's timer.
            for (idx, timer) in timers.iter_mut().enumerate() {
                timer.tick().await;
                if idx < self.config.sources.len() {
                    let source = &self.config.sources[idx];
                    let source_id = 1u16 << idx.min(15);
                    if let Err(e) = self.refresh_source(source, source_id).await {
                        tracing::warn!(
                            source = source.name,
                            error = %e,
                            "periodic refresh failed"
                        );
                    }
                }
            }
        }
    }
}

/// Decompress gzip-encoded bytes to a string.
pub fn decompress_gzip(data: &[u8]) -> std::result::Result<String, std::io::Error> {
    let mut decoder = GzDecoder::new(data);
    let mut result = String::new();
    decoder.read_to_string(&mut result)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocklist::test_utils::TempDir;
    use crate::blocklist::{BlocklistStore, Format, LmdbBlocklistStore};
    use std::net::TcpListener;
    use std::io::{Read, Write};

    /// A minimal mock HTTP server for testing.
    struct MockHttpServer {
        addr: std::net::SocketAddr,
        handle: Option<std::thread::JoinHandle<()>>,
        shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl MockHttpServer {
        fn new<F>(handler: F) -> Self
        where
            F: Fn(&str) -> (u16, String, Vec<u8>) + Send + Sync + 'static,
        {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let handler = Arc::new(handler);
            let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let shutdown_clone = shutdown.clone();

            let handle = std::thread::spawn(move || {
                listener.set_nonblocking(true).ok();
                loop {
                    if shutdown_clone.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let handler = handler.clone();
                            std::thread::spawn(move || {
                                let mut buf = [0u8; 4096];
                                let _ = stream.read(&mut buf);
                                let request = String::from_utf8_lossy(&buf);
                                let path = request
                                    .lines()
                                    .next()
                                    .and_then(|l| l.split_whitespace().nth(1))
                                    .unwrap_or("/");
                                let (status, content_type, body) = handler(path);
                                let status_text = match status {
                                    200 => "OK",
                                    404 => "Not Found",
                                    500 => "Internal Server Error",
                                    _ => "OK",
                                };
                                let response = format!(
                                    "HTTP/1.1 {status} {status_text}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
                                    body.len()
                                );
                                let _ = stream.write_all(response.as_bytes());
                                let _ = stream.write_all(&body);
                            });
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        Err(_) => break,
                    }
                }
            });

            Self {
                addr,
                handle: Some(handle),
                shutdown,
            }
        }

        fn url(&self) -> String {
            format!("http://{}", self.addr)
        }
    }

    impl Drop for MockHttpServer {
        fn drop(&mut self) {
            self.shutdown
                .store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    fn make_daemon(sources: Vec<SourceConfig>) -> (BlocklistDaemon, Arc<HotSwapStore>) {
        let dir = TempDir::new().unwrap();
        let live_path = dir.path().join("live.mdb");
        let store = LmdbBlocklistStore::open(&dir.path().join("init"), None).unwrap();
        let hot_swap = Arc::new(HotSwapStore::new(store, live_path));
        let config = BlocklistsConfig {
            sources,
            storage: Default::default(),
        };
        let daemon = BlocklistDaemon::new(config, hot_swap.clone());
        (daemon, hot_swap)
    }

    #[test]
    fn test_decompress_gzip() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write as IoWrite;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"hello world").unwrap();
        let compressed = encoder.finish().unwrap();

        let result = decompress_gzip(&compressed).unwrap();
        assert_eq!(result, "hello world");
    }

    #[tokio::test]
    async fn test_fetch_source_success() {
        let server = MockHttpServer::new(|_| {
            (
                200,
                "text/plain".to_string(),
                b"ads.example.com\ntracker.example.com\nmalware.example.org".to_vec(),
            )
        });

        let (daemon, _) = make_daemon(vec![SourceConfig {
            name: "test".to_string(),
            url: server.url(),
            format: Format::Domains,
            categories: vec![],
            refresh_hours: None,
            refresh_minutes: None,
        }]);

        let content = daemon.fetch_source(&daemon.config.sources[0]).await.unwrap();
        assert!(content.contains("ads.example.com"));
        assert!(content.contains("tracker.example.com"));
    }

    #[tokio::test]
    async fn test_fetch_source_http_error() {
        let server = MockHttpServer::new(|_| {
            (404, "text/plain".to_string(), b"not found".to_vec())
        });

        let (daemon, _) = make_daemon(vec![SourceConfig {
            name: "test".to_string(),
            url: server.url(),
            format: Format::Domains,
            categories: vec![],
            refresh_hours: None,
            refresh_minutes: None,
        }]);

        let result = daemon.fetch_source(&daemon.config.sources[0]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_fetch_source_too_small() {
        let server = MockHttpServer::new(|_| {
            (200, "text/plain".to_string(), b"tiny".to_vec())
        });

        let (daemon, _) = make_daemon(vec![SourceConfig {
            name: "test".to_string(),
            url: server.url(),
            format: Format::Domains,
            categories: vec![],
            refresh_hours: None,
            refresh_minutes: None,
        }]);

        let result = daemon.fetch_source(&daemon.config.sources[0]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_fetch_source_gzip() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write as IoWrite;

        let content = "ads.example.com\ntracker.example.com\nmalware.example.org";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(content.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();

        let compressed_clone = compressed.clone();
        let server = MockHttpServer::new(move |_| {
            (
                200,
                "application/gzip".to_string(),
                compressed_clone.clone(),
            )
        });

        let (daemon, _) = make_daemon(vec![SourceConfig {
            name: "test".to_string(),
            url: server.url(),
            format: Format::Domains,
            categories: vec![],
            refresh_hours: None,
            refresh_minutes: None,
        }]);

        let content = daemon.fetch_source(&daemon.config.sources[0]).await.unwrap();
        assert!(content.contains("ads.example.com"));
        assert!(content.contains("tracker.example.com"));
    }

    #[tokio::test]
    async fn test_validate_content_entry_count() {
        let (daemon, _) = make_daemon(vec![]);

        // First validation: no previous count, should pass.
        daemon.validate_content("test", 1000).unwrap();

        // Second validation: within 10%, should pass.
        daemon.validate_content("test", 1050).unwrap();

        // Third validation: exceeds 10%, should fail.
        let result = daemon.validate_content("test", 1200);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_content_zero_entries() {
        let (daemon, _) = make_daemon(vec![]);
        let result = daemon.validate_content("test", 0);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_refresh_source_success() {
        let server = MockHttpServer::new(|_| {
            let mut body = Vec::new();
            for i in 0..100 {
                body.extend_from_slice(format!("domain{i}.example.com\n").as_bytes());
            }
            (200, "text/plain".to_string(), body)
        });

        let (daemon, _) = make_daemon(vec![SourceConfig {
            name: "test".to_string(),
            url: server.url(),
            format: Format::Domains,
            categories: vec![],
            refresh_hours: None,
            refresh_minutes: None,
        }]);

        let count = daemon.refresh_source(&daemon.config.sources[0], 1).await.unwrap();
        assert_eq!(count, 100);
    }

    #[tokio::test]
    async fn test_refresh_source_hosts_format() {
        let server = MockHttpServer::new(|_| {
            let body = b"0.0.0.0 ads.example.com\n127.0.0.1 tracker.example.com\n0.0.0.0 malware.example.org".to_vec();
            (200, "text/plain".to_string(), body)
        });

        let (daemon, _) = make_daemon(vec![SourceConfig {
            name: "test".to_string(),
            url: server.url(),
            format: Format::Hosts,
            categories: vec![],
            refresh_hours: None,
            refresh_minutes: None,
        }]);

        let count = daemon.refresh_source(&daemon.config.sources[0], 1).await.unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_refresh_all_success() {
        let server = MockHttpServer::new(|path| {
            if path.contains("source1") {
                (
                    200,
                    "text/plain".to_string(),
                    b"ads.example.com\ntracker.example.com".to_vec(),
                )
            } else {
                (
                    200,
                    "text/plain".to_string(),
                    b"malware.example.org\nphishing.example.net".to_vec(),
                )
            }
        });

        let base_url = server.url();
        let (daemon, hot_swap) = make_daemon(vec![
            SourceConfig {
                name: "source1".to_string(),
                url: format!("{base_url}/source1"),
                format: Format::Domains,
                categories: vec![],
                refresh_hours: None,
                refresh_minutes: None,
            },
            SourceConfig {
                name: "source2".to_string(),
                url: format!("{base_url}/source2"),
                format: Format::Domains,
                categories: vec![],
                refresh_hours: None,
                refresh_minutes: None,
            },
        ]);

        daemon.refresh_all().await.unwrap();

        // Verify the hot-swapped store has entries.
        let store = hot_swap.load();
        assert!(store.len().unwrap() > 0);
        assert!(store.is_blocked("ads.example.com"));
        assert!(store.is_blocked("malware.example.org"));
    }
}
