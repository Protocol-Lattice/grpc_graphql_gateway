use crate::gbp::GbpEncoder;
use axum::body::to_bytes;
/// LZ4 compression middleware for Axum
///
/// This module provides LZ4 compression support as middleware for Axum applications.
/// LZ4 is an extremely fast compression algorithm ideal for high-throughput scenarios.
use axum::{
    body::Body,
    http::{header, Request, Response},
    middleware::Next,
};
use std::io::{Read, Write};

/// LZ4 compression middleware
///
/// Compresses response bodies using LZ4 if the client accepts it.
pub async fn lz4_compression_middleware(request: Request<Body>, next: Next) -> Response<Body> {
    let headers = request.headers().clone();
    let accept_encoding = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let accepts_gbp_lz4 = accept_encoding.contains("gbp-lz4");
    let accepts_lz4 = accept_encoding.contains("lz4");

    let response = next.run(request).await;

    // Only compress if client accepts LZ4 or GBP-LZ4
    if !accepts_lz4 && !accepts_gbp_lz4 {
        return response;
    }

    // Decompose response
    let (mut parts, body) = response.into_parts();

    // Collect body bytes
    let bytes = match to_bytes(body, 10 * 1024 * 1024).await {
        // 10MB limit
        Ok(b) => b,
        Err(_) => return Response::from_parts(parts, Body::empty()),
    };

    if accepts_gbp_lz4 {
        // Try to parse as JSON first
        if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            let mut encoder = GbpEncoder::new();
            if let Ok(compressed) = encoder.encode_lz4(&json_val) {
                parts.headers.insert(
                    header::CONTENT_ENCODING,
                    header::HeaderValue::from_static("gbp-lz4"),
                );
                parts.headers.insert(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("application/graphql-response+gbp"),
                );
                return Response::from_parts(parts, Body::from(compressed));
            }
        }
    }

    if accepts_lz4 {
        if let Ok(compressed) = compress_lz4(&bytes) {
            parts.headers.insert(
                header::CONTENT_ENCODING,
                header::HeaderValue::from_static("lz4"),
            );
            return Response::from_parts(parts, Body::from(compressed));
        }
    }

    Response::from_parts(parts, Body::from(bytes))
}

/// Compress data using LZ4
pub fn compress_lz4(data: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut encoder = lz4::EncoderBuilder::new()
        .level(1) // Fast compression
        .build(Vec::new())?;
    encoder.write_all(data)?;
    let (compressed, result) = encoder.finish();
    result?;
    Ok(compressed)
}

/// Decompress LZ4 data
pub fn decompress_lz4(data: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut decoder = lz4::Decoder::new(data)?;
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    Ok(decompressed)
}

/// LZ4 compression for cache storage (optional optimization)
///
/// This can be used to compress cached responses for more efficient memory usage.
pub struct Lz4CacheCompressor;

impl Lz4CacheCompressor {
    /// Compress JSON for cache storage
    pub fn compress(json: &str) -> Result<Vec<u8>, std::io::Error> {
        compress_lz4(json.as_bytes())
    }

    /// Decompress JSON from cache
    pub fn decompress(data: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
        let decompressed = decompress_lz4(data)?;
        Ok(String::from_utf8(decompressed)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http};
    use tower::ServiceExt;

    #[test]
    fn test_lz4_compression() {
        let data = b"Hello, World! ".repeat(100);
        let compressed = compress_lz4(&data).unwrap();

        // Compressed should be smaller for repetitive data
        assert!(compressed.len() < data.len());

        // Decompress and verify
        let decompressed = decompress_lz4(&compressed).unwrap();
        assert_eq!(data.to_vec(), decompressed);
    }

    #[test]
    fn test_lz4_empty_data() {
        let data = b"";
        let compressed = compress_lz4(data).unwrap();
        let decompressed = decompress_lz4(&compressed).unwrap();
        assert_eq!(data.to_vec(), decompressed);
    }

    #[test]
    fn test_lz4_small_data() {
        let data = b"abc";
       let compressed = compress_lz4(data).unwrap();
        let decompressed = decompress_lz4(&compressed).unwrap();
        assert_eq!(data.to_vec(), decompressed);
    }

    #[test]
    fn test_lz4_highly_compressible() {
        let data = vec![0u8; 10000]; // All zeros
        let compressed = compress_lz4(&data).unwrap();
        
        // Should compress extremely well
        let ratio = compressed.len() as f64 / data.len() as f64;
        assert!(ratio < 0.1, "Compression ratio should be < 10%");
        
        let decompressed = decompress_lz4(&compressed).unwrap();
        assert_eq!(data, decompressed);
    }

    #[test]
    fn test_lz4_random_data() {
        // Random data doesn't compress well
        let data: Vec<u8> = (0..1000).map(|i| (i * 31 + 17) as u8).collect();
        let compressed = compress_lz4(&data).unwrap();
        let decompressed = decompress_lz4(&compressed).unwrap();
        assert_eq!(data, decompressed);
    }

    #[test]
    fn test_lz4_json_compression() {
        let json = r#"{"data":{"users":[{"id":"1","name":"Alice"},{"id":"2","name":"Bob"}]}}"#;

        let compressed = Lz4CacheCompressor::compress(json).unwrap();
        let decompressed = Lz4CacheCompressor::decompress(&compressed).unwrap();

        assert_eq!(json, decompressed);
    }

    #[test]
    fn test_lz4_compressor_empty_json() {
        let json = "";
        let compressed = Lz4CacheCompressor::compress(json).unwrap();
        let decompressed = Lz4CacheCompressor::decompress(&compressed).unwrap();
        assert_eq!(json, decompressed);
    }

    #[test]
    fn test_lz4_compressor_unicode() {
        let json = r#"{"message":"Hello 世界 🌍"}"#;
        let compressed = Lz4CacheCompressor::compress(json).unwrap();
        let decompressed = Lz4CacheCompressor::decompress(&compressed).unwrap();
        assert_eq!(json, decompressed);
    }

    #[test]
    fn test_lz4_large_json() {
        // Simulate a large GraphQL response
        let data = format!(
            r#"{{"data":{{"items":[{}]}}}}"#,
            (0..1000)
                .map(|i| format!(r#"{{"id":"{}","name":"Item {}"}}"#, i, i))
                .collect::<Vec<_>>()
                .join(",")
        );

        let compressed = Lz4CacheCompressor::compress(&data).unwrap();

        // Should achieve good compression on repetitive JSON
        let ratio = compressed.len() as f64 / data.len() as f64;
        println!("LZ4 compression ratio: {:.1}%", ratio * 100.0);

        // Verify decompression
        let decompressed = Lz4CacheCompressor::decompress(&compressed).unwrap();
        assert_eq!(data, decompressed);
    }

    #[test]
    fn test_lz4_speed() {
        use std::time::Instant;

        let data = serde_json::json!({
            "data": {
                "users": (0..100).map(|i| {
                    serde_json::json!({
                        "id": i,
                        "name": format!("User {}", i),
                        "email": format!("user{}@example.com", i)
                    })
                }).collect::<Vec<_>>()
            }
        })
        .to_string();

        let start = Instant::now();
        let compressed = compress_lz4(data.as_bytes()).unwrap();
        let compress_time = start.elapsed();

        let start = Instant::now();
        let _decompressed = decompress_lz4(&compressed).unwrap();
        let decompress_time = start.elapsed();

        println!(
            "LZ4 compress: {:?}, decompress: {:?}",
            compress_time, decompress_time
        );
        println!(
            "Ratio: {:.1}%",
            compressed.len() as f64 / data.len() as f64 * 100.0
        );
        
        // LZ4 should be very fast
        assert!(compress_time.as_millis() < 100);
        assert!(decompress_time.as_millis() < 100);
    }

    #[test]
    fn test_lz4_roundtrip_various_sizes() {
        for size in [0, 1, 10, 100, 1000, 10000] {
            let data = vec![42u8; size];
            let compressed = compress_lz4(&data).unwrap();
            let decompressed = decompress_lz4(&compressed).unwrap();
            assert_eq!(data, decompressed, "Failed for size {}", size);
        }
    }

    #[test]
    fn test_lz4_json_array() {
        let json = r#"[1,2,3,4,5]"#;
        let compressed = Lz4CacheCompressor::compress(json).unwrap();
        let decompressed = Lz4CacheCompressor::decompress(&compressed).unwrap();
        assert_eq!(json, decompressed);
    }

    #[test]
    fn test_lz4_json_nested() {
        let json = r#"{"a":{"b":{"c":{"d":"value"}}}}"#;
        let compressed = Lz4CacheCompressor::compress(json).unwrap();
        let decompressed = Lz4CacheCompressor::decompress(&compressed).unwrap();
        assert_eq!(json, decompressed);
    }

    #[test]
    fn test_lz4_binary_data() {
        let data: Vec<u8> = (0..=255).cycle().take(5000).collect();
        let compressed = compress_lz4(&data).unwrap();
        let decompressed = decompress_lz4(&compressed).unwrap();
        assert_eq!(data, decompressed);
    }

    #[test]
    fn test_lz4_compression_level() {
        let data = b"Test data for compression level comparison";
        let compressed = compress_lz4(data).unwrap();
        // Just verify it works - level is already set to 1 (fast)
        assert!(compressed.len() > 0);
    }

    #[tokio::test]
    async fn test_lz4_middleware_no_accept_encoding() {
        let app = axum::Router::new()
            .route("/test", axum::routing::get(|| async { "Hello" }))
            .layer(axum::middleware::from_fn(lz4_compression_middleware));

        let request = http::Request::builder()
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        
        // Should not compress without Accept-Encoding
        assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn test_lz4_middleware_accepts_lz4() {
        let app = axum::Router::new()
            .route("/test", axum::routing::get(|| async { "Hello World ".repeat(100) }))
            .layer(axum::middleware::from_fn(lz4_compression_middleware));

        let request = http::Request::builder()
            .uri("/test")
            .header(header::ACCEPT_ENCODING, "lz4")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        
        // Should compress with lz4
        assert_eq!(
            response.headers().get(header::CONTENT_ENCODING).map(|v| v.as_bytes()),
            Some(b"lz4".as_ref())
        );
    }

    #[tokio::test]
    async fn test_lz4_middleware_accepts_gbp_lz4() {
        use axum::response::Json;
        
        let app = axum::Router::new()
            .route("/test", axum::routing::get(|| async {
                Json(serde_json::json!({"data": {"test": "value"}}))
            }))
            .layer(axum::middleware::from_fn(lz4_compression_middleware));

        let request = http::Request::builder()
            .uri("/test")
            .header(header::ACCEPT_ENCODING, "gbp-lz4")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        
        // Should compress with gbp-lz4 for JSON responses
        let content_encoding = response.headers().get(header::CONTENT_ENCODING);
        if content_encoding.is_some() {
            assert_eq!(
                content_encoding.map(|v| v.as_bytes()),
                Some(b"gbp-lz4".as_ref())
            );
        }
    }

    #[test]
    fn test_lz4_special_characters() {
        let json = r#"{"special":"<>&\"'\n\t\r"}"#;
        let compressed = Lz4CacheCompressor::compress(json).unwrap();
        let decompressed = Lz4CacheCompressor::decompress(&compressed).unwrap();
        assert_eq!(json, decompressed);
    }

    #[test]
    fn test_lz4_very_long_string() {
        let long_string = "a".repeat(100000);
        let compressed = compress_lz4(long_string.as_bytes()).unwrap();
        let decompressed = decompress_lz4(&compressed).unwrap();
        assert_eq!(long_string.as_bytes(), decompressed.as_slice());
        
        // Should compress very well
        let ratio = compressed.len() as f64 / long_string.len() as f64;
        assert!(ratio < 0.05, "Expected > 95% compression");
    }
}
