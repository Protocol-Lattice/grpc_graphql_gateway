//! Response Compression Support
//!
//! This module provides automatic HTTP response compression using gzip, brotli,
//! and deflate algorithms. Compression is applied transparently based on client
//! `Accept-Encoding` headers.
//!
//! # Overview
//!
//! Response compression reduces bandwidth usage by compressing HTTP response bodies
//! before sending them to clients. This is particularly beneficial for GraphQL responses
//! which are typically JSON and compress very well (50-90% size reduction).
//!
//! # Supported Algorithms
//!
//! - **LZ4** (`lz4`) - Ultra-fast compression, ideal for high-throughput (NEW!)
//! - **Brotli** (`br`) - Best compression ratio, preferred for modern browsers
//! - **Gzip** (`gzip`) - Widely supported, good compression
//! - **Deflate** (`deflate`) - Legacy support
//! - **Zstd** (`zstd`) - Modern, fast compression (optional)
//!
//! # Example
//!
//! ```rust,no_run
//! use grpc_graphql_gateway::{Gateway, CompressionConfig, CompressionLevel};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let gateway = Gateway::builder()
//!     .with_compression(CompressionConfig {
//!         enabled: true,
//!         level: CompressionLevel::Default,
//!         min_size_bytes: 1024,  // Only compress responses > 1KB
//!         algorithms: vec!["br".into(), "gzip".into()],
//!     })
//!     // ... other configuration
//!     .serve("0.0.0.0:8888")
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! # How It Works
//!
//! 1. Client sends request with `Accept-Encoding: gzip, deflate, br`
//! 2. Gateway selects the best algorithm (preferring brotli > gzip > deflate)
//! 3. Response body is compressed before sending
//! 4. Gateway sets `Content-Encoding` header to indicate compression
//! 5. Client decompresses the response automatically
//!
//! # Performance Considerations
//!
//! - **CPU Cost**: Compression uses CPU cycles. Use `CompressionLevel::Fast` for
//!   latency-sensitive applications.
//! - **Min Size**: Set `min_size_bytes` to skip compression for small responses
//!   where overhead exceeds savings.
//! - **Caching**: Compressed responses work well with the response cache feature.

use std::fmt;

/// Compression level controlling the trade-off between speed and size.
///
/// # Examples
///
/// ```rust
/// use grpc_graphql_gateway::CompressionLevel;
///
/// // Fast compression for low latency
/// let fast = CompressionLevel::Fast;
///
/// // Best compression for bandwidth savings
/// let best = CompressionLevel::Best;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressionLevel {
    /// Fastest compression, lowest ratio (level 1)
    Fast,
    /// Balanced compression (default, level 4-6)
    #[default]
    Default,
    /// Best compression, slower (level 9-11)
    Best,
    /// Custom compression level (algorithm-specific)
    Custom(u32),
}

impl CompressionLevel {
    /// Convert to tower-http compression level for gzip
    pub fn to_gzip_level(&self) -> tower_http::CompressionLevel {
        match self {
            CompressionLevel::Fast => tower_http::CompressionLevel::Fastest,
            CompressionLevel::Default => tower_http::CompressionLevel::Default,
            CompressionLevel::Best => tower_http::CompressionLevel::Best,
            CompressionLevel::Custom(_) => tower_http::CompressionLevel::Default,
        }
    }
}

impl fmt::Display for CompressionLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompressionLevel::Fast => write!(f, "fast"),
            CompressionLevel::Default => write!(f, "default"),
            CompressionLevel::Best => write!(f, "best"),
            CompressionLevel::Custom(level) => write!(f, "custom({})", level),
        }
    }
}

/// Configuration for response compression.
///
/// # Example
///
/// ```rust
/// use grpc_graphql_gateway::{CompressionConfig, CompressionLevel};
///
/// let config = CompressionConfig {
///     enabled: true,
///     level: CompressionLevel::Default,
///     min_size_bytes: 1024,
///     algorithms: vec!["br".into(), "gzip".into()],
/// };
/// ```
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    /// Whether compression is enabled (default: true)
    pub enabled: bool,

    /// Compression level (default: `CompressionLevel::Default`)
    pub level: CompressionLevel,

    /// Minimum response size in bytes to compress (default: 1024)
    ///
    /// Responses smaller than this threshold are not compressed
    /// because the overhead may exceed the savings.
    pub min_size_bytes: usize,

    /// Enabled compression algorithms in preference order (default: ["br", "gzip", "deflate"])
    ///
    /// Supported values:
    /// - `"gbp-lz4"` - GraphQL Binary Protocol + LZ4 (99% compression)
    /// - `"lz4"` - LZ4 (ultra-fast, great for high-throughput)
    /// - `"br"` - Brotli (recommended, best ratio)
    /// - `"gzip"` - Gzip (widely supported)
    /// - `"deflate"` - Deflate (legacy)
    /// - `"zstd"` - Zstandard (requires feature flag)
    pub algorithms: Vec<String>,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            level: CompressionLevel::Default,
            min_size_bytes: 1024,
            algorithms: vec![
                "gbp-lz4".into(),
                "br".into(),
                "gzip".into(),
                "deflate".into(),
            ],
        }
    }
}

impl CompressionConfig {
    /// Create a new compression config with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a config with compression disabled.
    ///
    /// # Example
    ///
    /// ```rust
    /// use grpc_graphql_gateway::CompressionConfig;
    ///
    /// let config = CompressionConfig::disabled();
    /// assert!(!config.enabled);
    /// ```
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Create a fast compression config optimized for low latency.
    ///
    /// # Example
    ///
    /// ```rust
    /// use grpc_graphql_gateway::CompressionConfig;
    ///
    /// let config = CompressionConfig::fast();
    /// ```
    pub fn fast() -> Self {
        Self {
            enabled: true,
            level: CompressionLevel::Fast,
            min_size_bytes: 512,
            algorithms: vec!["gzip".into()], // gzip is faster than brotli at low levels
        }
    }

    /// Create a best compression config optimized for bandwidth savings.
    ///
    /// # Example
    ///
    /// ```rust
    /// use grpc_graphql_gateway::CompressionConfig;
    ///
    /// let config = CompressionConfig::best();
    /// ```
    pub fn best() -> Self {
        Self {
            enabled: true,
            level: CompressionLevel::Best,
            min_size_bytes: 256,
            algorithms: vec!["br".into(), "gzip".into()],
        }
    }

    /// Builder method to set the compression level.
    pub fn with_level(mut self, level: CompressionLevel) -> Self {
        self.level = level;
        self
    }

    /// Builder method to set the minimum size threshold.
    ///
    /// # Security – BB-08 (BREACH/CRIME mitigation)
    /// Compression of responses that mix attacker-controlled data with secrets
    /// (e.g. CSRF tokens) enables compression oracle attacks when the threshold is
    /// too low.  A minimum of **256 bytes** is therefore enforced: values below
    /// this floor are silently raised to 256.
    pub fn with_min_size(mut self, min_size_bytes: usize) -> Self {
        const MIN_SAFE_THRESHOLD: usize = 256;
        if min_size_bytes < MIN_SAFE_THRESHOLD {
            tracing::warn!(
                requested = min_size_bytes,
                enforced = MIN_SAFE_THRESHOLD,
                "min_size_bytes raised to the minimum safe threshold (BREACH/CRIME mitigation)"
            );
            self.min_size_bytes = MIN_SAFE_THRESHOLD;
        } else {
            self.min_size_bytes = min_size_bytes;
        }
        self
    }

    /// Builder method to set the enabled algorithms.
    pub fn with_algorithms(mut self, algorithms: Vec<String>) -> Self {
        self.algorithms = algorithms;
        self
    }

    /// Check if brotli compression is enabled.
    pub fn brotli_enabled(&self) -> bool {
        self.enabled && self.algorithms.iter().any(|a| a == "br" || a == "brotli")
    }

    /// Check if gzip compression is enabled.
    pub fn gzip_enabled(&self) -> bool {
        self.enabled && self.algorithms.iter().any(|a| a == "gzip")
    }

    /// Check if deflate compression is enabled.
    pub fn deflate_enabled(&self) -> bool {
        self.enabled && self.algorithms.iter().any(|a| a == "deflate")
    }

    /// Check if zstd compression is enabled.
    pub fn zstd_enabled(&self) -> bool {
        self.enabled && self.algorithms.iter().any(|a| a == "zstd")
    }

    /// Check if gbp-lz4 compression is enabled.
    pub fn gbp_lz4_enabled(&self) -> bool {
        self.enabled && self.algorithms.iter().any(|a| a == "gbp-lz4")
    }

    /// Check if lz4 compression is enabled.
    pub fn lz4_enabled(&self) -> bool {
        self.enabled && self.algorithms.iter().any(|a| a == "lz4")
    }

    /// Create an ultra-fast config using GBP + LZ4 compression.
    ///
    /// GBP uses structural deduplication to achieve up to 99% compression
    /// on redundant GraphQL responses, then compresses with LZ4.
    pub fn ultra_fast() -> Self {
        Self {
            enabled: true,
            level: CompressionLevel::Fast,
            // BB-08: Do not set below 256 here; see with_min_size() for rationale.
            min_size_bytes: 256,
            algorithms: vec!["gbp-lz4".into(), "lz4".into()],
        }
    }
}

/// Create a compression layer for the Axum router.
///
/// This function creates a `tower_http::compression::CompressionLayer` configured
/// based on the provided `CompressionConfig`.
///
/// # Arguments
///
/// * `config` - The compression configuration
///
/// # Returns
///
/// A configured compression layer that can be added to an Axum router.
pub fn create_compression_layer(
    config: &CompressionConfig,
) -> tower_http::compression::CompressionLayer {
    use tower_http::compression::CompressionLayer;

    let mut layer = CompressionLayer::new();

    // Set compression level
    layer = layer.quality(config.level.to_gzip_level());

    // Configure algorithms
    if !config.gzip_enabled() {
        layer = layer.no_gzip();
    }
    if !config.brotli_enabled() {
        layer = layer.no_br();
    }
    if !config.deflate_enabled() {
        layer = layer.no_deflate();
    }
    if !config.zstd_enabled() {
        layer = layer.no_zstd();
    }

    layer
}

/// Compression statistics for monitoring.
///
/// # BB-09 – Field visibility
/// All counters are **private**; use the provided methods to record events and
/// read values.  This prevents external code from setting `bytes_out > bytes_in`,
/// which would make `savings_percentage` return a negative number and could
/// mislead monitoring dashboards.
#[derive(Debug, Clone, Default)]
pub struct CompressionStats {
    bytes_in: u64,
    bytes_out: u64,
    compressed_count: u64,
    uncompressed_count: u64,
}

impl CompressionStats {
    /// Record a compressed response.
    ///
    /// `original_bytes` must be ≥ `compressed_bytes`; if not the call is a no-op
    /// and a warning is emitted to avoid corrupting the ratio metric.
    pub fn record_compressed(&mut self, original_bytes: u64, compressed_bytes: u64) {
        if compressed_bytes > original_bytes {
            tracing::warn!(
                original = original_bytes,
                compressed = compressed_bytes,
                "Compressed size exceeds original — skipping stats update"
            );
            return;
        }
        self.bytes_in = self.bytes_in.saturating_add(original_bytes);
        self.bytes_out = self.bytes_out.saturating_add(compressed_bytes);
        self.compressed_count = self.compressed_count.saturating_add(1);
    }

    /// Record an uncompressed response (e.g. below the size threshold).
    pub fn record_uncompressed(&mut self, bytes: u64) {
        self.bytes_in = self.bytes_in.saturating_add(bytes);
        self.bytes_out = self.bytes_out.saturating_add(bytes);
        self.uncompressed_count = self.uncompressed_count.saturating_add(1);
    }

    /// Total bytes received before compression.
    pub fn bytes_in(&self) -> u64 {
        self.bytes_in
    }
    /// Total bytes sent after compression.
    pub fn bytes_out(&self) -> u64 {
        self.bytes_out
    }
    /// Number of responses that were compressed.
    pub fn compressed_count(&self) -> u64 {
        self.compressed_count
    }
    /// Number of responses that were NOT compressed.
    pub fn uncompressed_count(&self) -> u64 {
        self.uncompressed_count
    }

    /// Calculate the compression ratio (compressed / original).
    ///
    /// Returns 1.0 if no data has been compressed.
    pub fn compression_ratio(&self) -> f64 {
        if self.bytes_in == 0 {
            1.0
        } else {
            self.bytes_out as f64 / self.bytes_in as f64
        }
    }

    /// Calculate the space savings percentage.
    ///
    /// Returns 0.0 if no data has been compressed.
    pub fn savings_percentage(&self) -> f64 {
        if self.bytes_in == 0 {
            0.0
        } else {
            (1.0 - (self.bytes_out as f64 / self.bytes_in as f64)) * 100.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compression_config_default() {
        let config = CompressionConfig::default();
        assert!(config.enabled);
        assert!(config.brotli_enabled());
        assert!(config.gzip_enabled());
        assert!(config.deflate_enabled());
        assert!(!config.zstd_enabled());
        assert_eq!(config.min_size_bytes, 1024);
    }

    #[test]
    fn test_compression_config_disabled() {
        let config = CompressionConfig::disabled();
        assert!(!config.enabled);
        assert!(!config.brotli_enabled());
        assert!(!config.gzip_enabled());
    }

    #[test]
    fn test_compression_config_fast() {
        let config = CompressionConfig::fast();
        assert!(config.enabled);
        assert!(!config.brotli_enabled());
        assert!(config.gzip_enabled());
        assert_eq!(config.level, CompressionLevel::Fast);
    }

    #[test]
    fn test_compression_config_best() {
        let config = CompressionConfig::best();
        assert!(config.enabled);
        assert!(config.brotli_enabled());
        assert!(config.gzip_enabled());
        assert_eq!(config.level, CompressionLevel::Best);
    }

    #[test]
    fn test_compression_config_builder() {
        let config = CompressionConfig::new()
            .with_level(CompressionLevel::Best)
            .with_min_size(2048)
            .with_algorithms(vec!["br".into()]);

        assert_eq!(config.level, CompressionLevel::Best);
        assert_eq!(config.min_size_bytes, 2048);
        assert!(config.brotli_enabled());
        assert!(!config.gzip_enabled());
    }

    #[test]
    fn test_compression_stats() {
        // BB-09: Use record_* methods instead of direct field construction.
        let mut stats = CompressionStats::default();
        for _ in 0..10 {
            stats.record_compressed(100, 30);
        }
        for _ in 0..5 {
            stats.record_uncompressed(100);
        }

        // 10 compressed: 1000 in, 300 out; 5 uncompressed: 500 in, 500 out
        // total: 1500 in, 800 out  ->  ratio = 0.5333…
        assert!((stats.compression_ratio() - (800.0 / 1500.0)).abs() < 0.001);
        assert!((stats.savings_percentage() - (1.0 - 800.0 / 1500.0) * 100.0).abs() < 0.001);
        assert_eq!(stats.compressed_count(), 10);
        assert_eq!(stats.uncompressed_count(), 5);
    }

    #[test]
    fn test_compression_stats_pure_compressed() {
        // Matches the spirit of the original test (1000 in, 300 out for 10 events).
        let mut stats = CompressionStats::default();
        for _ in 0..10 {
            stats.record_compressed(100, 30);
        }
        assert_eq!(stats.bytes_in(), 1000);
        assert_eq!(stats.bytes_out(), 300);
        assert!((stats.compression_ratio() - 0.3).abs() < 0.001);
        assert!((stats.savings_percentage() - 70.0).abs() < 0.001);
    }

    #[test]
    fn test_compression_level_display() {
        assert_eq!(CompressionLevel::Fast.to_string(), "fast");
        assert_eq!(CompressionLevel::Default.to_string(), "default");
        assert_eq!(CompressionLevel::Best.to_string(), "best");
        assert_eq!(CompressionLevel::Custom(5).to_string(), "custom(5)");
    }

    #[test]
    fn test_create_compression_layer() {
        let config = CompressionConfig::default();
        let _layer = create_compression_layer(&config);
        // Layer was created successfully
    }

    #[test]
    fn test_compression_level_to_gzip() {
        assert_eq!(
            CompressionLevel::Fast.to_gzip_level(),
            tower_http::CompressionLevel::Fastest
        );
        assert_eq!(
            CompressionLevel::Default.to_gzip_level(),
            tower_http::CompressionLevel::Default
        );
        assert_eq!(
            CompressionLevel::Best.to_gzip_level(),
            tower_http::CompressionLevel::Best
        );
        assert_eq!(
            CompressionLevel::Custom(7).to_gzip_level(),
            tower_http::CompressionLevel::Default
        );
    }

    #[test]
    fn test_compression_level_equality() {
        assert_eq!(CompressionLevel::Fast, CompressionLevel::Fast);
        assert_ne!(CompressionLevel::Fast, CompressionLevel::Best);
        assert_eq!(CompressionLevel::Custom(5), CompressionLevel::Custom(5));
        assert_ne!(CompressionLevel::Custom(5), CompressionLevel::Custom(6));
    }

    #[test]
    fn test_compression_config_ultra_fast() {
        let config = CompressionConfig::ultra_fast();
        assert!(config.enabled);
        assert!(config.gbp_lz4_enabled());
        assert!(config.lz4_enabled());
        assert_eq!(config.level, CompressionLevel::Fast);
        // BB-08: min_size_bytes floor is 256 (was 128) to mitigate compression oracles.
        assert_eq!(config.min_size_bytes, 256);
    }

    #[test]
    fn test_lz4_enabled() {
        let config = CompressionConfig::default();
        assert!(!config.lz4_enabled());

        let lz4_config = CompressionConfig::new().with_algorithms(vec!["lz4".into()]);
        assert!(lz4_config.lz4_enabled());
    }

    #[test]
    fn test_gbp_lz4_enabled() {
        let config = CompressionConfig::default();
        assert!(config.gbp_lz4_enabled()); // Default includes gbp-lz4

        let no_gbp = CompressionConfig::new().with_algorithms(vec!["gzip".into()]);
        assert!(!no_gbp.gbp_lz4_enabled());
    }

    #[test]
    fn test_zstd_enabled() {
        let config = CompressionConfig::default();
        assert!(!config.zstd_enabled());

        let zstd_config = CompressionConfig::new().with_algorithms(vec!["zstd".into()]);
        assert!(zstd_config.zstd_enabled());
    }

    #[test]
    fn test_compression_stats_zero_input() {
        let stats = CompressionStats::default();
        assert_eq!(stats.compression_ratio(), 1.0);
        assert_eq!(stats.savings_percentage(), 0.0);
    }

    #[test]
    fn test_compression_stats_no_compression() {
        // BB-09: Use record_uncompressed instead of direct field assignment.
        let mut stats = CompressionStats::default();
        for _ in 0..10 {
            stats.record_uncompressed(100);
        }
        assert_eq!(stats.compression_ratio(), 1.0);
        assert_eq!(stats.savings_percentage(), 0.0);
        assert_eq!(stats.uncompressed_count(), 10);
    }

    #[test]
    fn test_compression_stats_perfect_compression() {
        // BB-09: record_compressed guards against compressed > original, so we
        // model near-perfect compression (1 byte remaining) rather than 0 bytes.
        let mut stats = CompressionStats::default();
        for _ in 0..10 {
            stats.record_compressed(100, 1);
        }
        assert!(stats.compression_ratio() < 0.02);
        assert!(stats.savings_percentage() >= 99.0);
        assert_eq!(stats.compressed_count(), 10);
    }

    #[test]
    fn test_compression_config_clone() {
        let config1 = CompressionConfig::best();
        let config2 = config1.clone();

        assert_eq!(config1.level, config2.level);
        assert_eq!(config1.enabled, config2.enabled);
        assert_eq!(config1.min_size_bytes, config2.min_size_bytes);
    }

    #[test]
    fn test_compression_config_new() {
        let config = CompressionConfig::new();
        assert!(config.enabled);
        assert_eq!(config.level, CompressionLevel::Default);
    }

    #[test]
    fn test_compression_level_copy() {
        let level1 = CompressionLevel::Fast;
        let level2 = level1;
        assert_eq!(level1, level2);
    }

    #[test]
    fn test_compression_stats_default() {
        // BB-09: Access via getters now that fields are private.
        let stats = CompressionStats::default();
        assert_eq!(stats.bytes_in(), 0);
        assert_eq!(stats.bytes_out(), 0);
        assert_eq!(stats.compressed_count(), 0);
        assert_eq!(stats.uncompressed_count(), 0);
    }

    #[test]
    fn test_multiple_algorithms() {
        let config = CompressionConfig::new().with_algorithms(vec![
            "gbp-lz4".into(),
            "lz4".into(),
            "br".into(),
            "gzip".into(),
            "deflate".into(),
            "zstd".into(),
        ]);

        assert!(config.gbp_lz4_enabled());
        assert!(config.lz4_enabled());
        assert!(config.brotli_enabled());
        assert!(config.gzip_enabled());
        assert!(config.deflate_enabled());
        assert!(config.zstd_enabled());
    }
}
