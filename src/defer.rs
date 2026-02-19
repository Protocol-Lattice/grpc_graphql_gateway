//! GraphQL `@defer` directive support for incremental delivery.
//!
//! This module enables the `@defer` directive in incoming GraphQL queries,
//! allowing the gateway to return an initial payload immediately and
//! stream deferred fragments as incremental patches.
//!
//! # Transport
//!
//! Deferred results are delivered using the `multipart/mixed` HTTP transport
//! as specified by the GraphQL incremental delivery RFC:
//! <https://github.com/graphql/graphql-spec/blob/main/rfcs/DeferStream.md>
//!
//! Each part is a JSON object separated by the multipart boundary.
//!
//! # Example
//!
//! ```graphql
//! query GetUser {
//!   user(id: "1") {
//!     id
//!     name
//!     ... @defer(label: "details") {
//!       email
//!       address
//!     }
//!   }
//! }
//! ```
//!
//! The response is streamed as:
//!
//! 1. **Initial payload** (fast): `{ "data": { "user": { "id": "1", "name": "Alice" } }, "hasNext": true }`
//! 2. **Deferred patch**: `{ "incremental": [{ "data": { "email": "...", "address": "..." }, "path": ["user"], "label": "details" }], "hasNext": false }`
//!
//! # Configuration
//!
//! ```rust,ignore
//! use grpc_graphql_gateway::defer::DeferConfig;
//!
//! let config = DeferConfig {
//!     enabled: true,
//!     max_deferred_fragments: 16,
//!     fragment_timeout_ms: 30_000,
//! };
//! ```

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, warn};

// ─── Configuration ─────────────────────────────────────────────────────

/// Configuration for `@defer` support.
#[derive(Debug, Clone)]
pub struct DeferConfig {
    /// Whether `@defer` is enabled (default: `true`).
    pub enabled: bool,
    /// Maximum number of deferred fragments per query (default: `16`).
    pub max_deferred_fragments: usize,
    /// Timeout for each deferred fragment resolution in milliseconds (default: `30_000`).
    pub fragment_timeout_ms: u64,
    /// Maximum total query time including all deferred fragments (ms, default: `60_000`).
    pub max_total_timeout_ms: u64,
    /// The multipart boundary string (default: `"-"`)
    pub multipart_boundary: String,
}

impl Default for DeferConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_deferred_fragments: 16,
            fragment_timeout_ms: 30_000,
            max_total_timeout_ms: 60_000,
            multipart_boundary: "-".to_string(),
        }
    }
}

impl DeferConfig {
    /// Create a strict configuration for production, with tight limits.
    pub fn production() -> Self {
        Self {
            enabled: true,
            max_deferred_fragments: 8,
            fragment_timeout_ms: 15_000,
            max_total_timeout_ms: 30_000,
            multipart_boundary: "-".to_string(),
        }
    }

    /// Create a permissive configuration for development.
    pub fn development() -> Self {
        Self {
            enabled: true,
            max_deferred_fragments: 64,
            fragment_timeout_ms: 60_000,
            max_total_timeout_ms: 120_000,
            multipart_boundary: "-".to_string(),
        }
    }

    /// Disabled configuration — queries with `@defer` are executed normally
    /// (all fields resolved before responding).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Get the fragment timeout as a `Duration`.
    pub fn fragment_timeout(&self) -> Duration {
        Duration::from_millis(self.fragment_timeout_ms)
    }

    /// Get the total timeout as a `Duration`.
    pub fn total_timeout(&self) -> Duration {
        Duration::from_millis(self.max_total_timeout_ms)
    }
}

// ─── Directive Detection / Stripping ───────────────────────────────────

/// Check whether a query string contains the `@defer` directive.
///
/// This scans the raw query text after stripping comments. It handles
/// both inline `... @defer { ... }` and `... @defer(label: "x") { ... }`.
pub fn has_defer_directive(query: &str) -> bool {
    let normalized = query
        .lines()
        .map(|line| {
            if let Some(idx) = line.find('#') {
                &line[..idx]
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let patterns = [
        "@defer ", "@defer\n", "@defer\t", "@defer(", "@defer{", "@defer\r",
    ];
    for pattern in patterns {
        if normalized.contains(pattern) {
            return true;
        }
    }
    // Check for @defer at end of string
    if normalized.trim_end().ends_with("@defer") {
        return true;
    }

    false
}

/// Strip all `@defer` directives from a query string.
///
/// This removes `@defer` and `@defer(...)` occurrences so that the
/// underlying schema executor (async-graphql) processes all fields
/// eagerly. The gateway then parcels the response into initial +
/// incremental parts.
pub fn strip_defer_directives(query: &str) -> String {
    let mut result = query.to_string();

    // Remove @defer(...) with arguments — greedy innermost-first
    loop {
        if let Some(start) = result.find("@defer(") {
            let after_open = start + 7; // length of "@defer("
            let mut depth = 1;
            let mut end = after_open;
            for (i, ch) in result[after_open..].char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = after_open + i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            result = format!("{}{}", &result[..start], &result[end..]);
        } else {
            break;
        }
    }

    // Remove bare @defer (no args)
    result = result.replace("@defer", "");

    result
}

// ─── Deferred Fragment Extraction ──────────────────────────────────────

/// Represents a single deferred fragment parsed from a query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeferredFragment {
    /// Optional label (from `@defer(label: "...")`)
    pub label: Option<String>,
    /// The JSON path where this fragment's data should be merged.
    /// e.g. `["user"]`, `["users", "0"]`.
    pub path: Vec<String>,
    /// Field names inside the deferred fragment.
    pub fields: Vec<String>,
    /// Whether this fragment was requested with `if: false` (i.e. not deferred).
    pub disabled: bool,
}

/// Extract deferred fragment metadata from a raw query string.
///
/// This performs a lightweight parse to identify `... @defer` spread sites
/// and extract their labels, paths, and fields. It is **not** a full
/// GraphQL parser; edge-cases with deeply nested aliases or unions may
/// require enhancement.
pub fn extract_deferred_fragments(query: &str) -> Vec<DeferredFragment> {
    let mut fragments = Vec::new();

    // Use regex to find @defer directive occurrences with optional args
    let defer_re = Regex::new(
        r"\.{3}\s*@defer(?:\(([^)]*)\))?\s*\{([^}]*)\}"
    ).unwrap();

    for cap in defer_re.captures_iter(query) {
        let args = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let body = cap.get(2).map(|m| m.as_str()).unwrap_or("");

        // Parse label from args
        let label = extract_arg(args, "label");

        // Parse if condition
        let disabled = extract_arg(args, "if")
            .map(|v| v == "false")
            .unwrap_or(false);

        // Extract field names from body (simple top-level field extraction)
        let fields: Vec<String> = body
            .split_whitespace()
            .filter(|s| !s.is_empty() && !s.starts_with('#'))
            .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric() && c != '_').to_string())
            .filter(|s| !s.is_empty())
            .collect();

        fragments.push(DeferredFragment {
            label,
            path: Vec::new(), // Path is determined at execution time
            fields,
            disabled,
        });
    }

    fragments
}

/// Extract a named argument value from a directive argument string.
/// e.g. `label: "details", if: true` → `extract_arg(s, "label")` = `Some("details")`
fn extract_arg(args: &str, name: &str) -> Option<String> {
    let pattern = format!("{name}:");
    if let Some(idx) = args.find(&pattern) {
        let after = &args[idx + pattern.len()..];
        let trimmed = after.trim_start();
        // Handle quoted strings
        if trimmed.starts_with('"') {
            let inner = &trimmed[1..];
            if let Some(end) = inner.find('"') {
                return Some(inner[..end].to_string());
            }
        }
        // Handle bare values (true, false, identifiers)
        let end = trimmed
            .find(|c: char| c == ',' || c == ')' || c.is_whitespace())
            .unwrap_or(trimmed.len());
        let val = trimmed[..end].to_string();
        if !val.is_empty() {
            return Some(val);
        }
    }
    None
}

// ─── Incremental Delivery Types ────────────────────────────────────────

/// The initial response payload when `@defer` is active.
#[derive(Debug, Clone, Serialize)]
pub struct InitialPayload {
    /// The eagerly-resolved data.
    pub data: serde_json::Value,
    /// GraphQL errors from the initial execution, if any.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<serde_json::Value>,
    /// `true` when there are pending deferred fragments.
    #[serde(rename = "hasNext")]
    pub has_next: bool,
}

/// An incremental patch delivered after the initial payload.
#[derive(Debug, Clone, Serialize)]
pub struct IncrementalPatch {
    /// Data for this deferred fragment.
    pub data: serde_json::Value,
    /// JSON path where the data should be merged into the initial response.
    pub path: Vec<serde_json::Value>,
    /// Optional label matching `@defer(label: "...")`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Errors specific to this deferred fragment.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<serde_json::Value>,
}

/// A subsequent payload containing incremental patches.
#[derive(Debug, Clone, Serialize)]
pub struct SubsequentPayload {
    /// The incremental patches.
    pub incremental: Vec<IncrementalPatch>,
    /// `false` when this is the final payload.
    #[serde(rename = "hasNext")]
    pub has_next: bool,
}

// ─── Multipart Response Formatting ─────────────────────────────────────

/// The content type for incremental delivery via multipart/mixed.
pub const MULTIPART_CONTENT_TYPE: &str =
    "multipart/mixed; boundary=\"-\"";

/// Format the initial payload as a multipart part.
pub fn format_initial_part(payload: &InitialPayload, boundary: &str) -> String {
    let json = serde_json::to_string(payload).unwrap_or_default();
    format!(
        "\r\n--{boundary}\r\nContent-Type: application/json; charset=utf-8\r\n\r\n{json}\r\n"
    )
}

/// Format a subsequent payload as a multipart part.
pub fn format_subsequent_part(payload: &SubsequentPayload, boundary: &str) -> String {
    let json = serde_json::to_string(payload).unwrap_or_default();
    if payload.has_next {
        format!(
            "--{boundary}\r\nContent-Type: application/json; charset=utf-8\r\n\r\n{json}\r\n"
        )
    } else {
        format!(
            "--{boundary}\r\nContent-Type: application/json; charset=utf-8\r\n\r\n{json}\r\n--{boundary}--\r\n"
        )
    }
}

// ─── Deferred Execution Engine ─────────────────────────────────────────

/// Execution context for a deferred query.
///
/// This manages the lifecycle of a single request that uses `@defer`:
/// 1. Execute the full query (with `@defer` stripped) eagerly
/// 2. Split the response into initial and deferred parts
/// 3. Stream deferred parts to the client
pub struct DeferredExecution {
    /// Configuration
    config: DeferConfig,
    /// Deferred fragments extracted from the query
    fragments: Vec<DeferredFragment>,
    /// Channel sender for streaming parts to the HTTP response
    tx: mpsc::Sender<DeferredPart>,
    /// Start time for timeout tracking
    started_at: Instant,
}

/// A part of the deferred response (either initial or incremental).
#[derive(Debug)]
pub enum DeferredPart {
    /// The initial, eagerly-resolved response.
    Initial(InitialPayload),
    /// An incremental patch for a deferred fragment.
    Subsequent(SubsequentPayload),
}

impl DeferredExecution {
    /// Create a new deferred execution context.
    ///
    /// Returns `(execution, receiver)` — the caller should consume the
    /// receiver to stream parts to the client.
    pub fn new(
        config: DeferConfig,
        fragments: Vec<DeferredFragment>,
    ) -> (Self, mpsc::Receiver<DeferredPart>) {
        let (tx, rx) = mpsc::channel(fragments.len() + 2);
        (
            Self {
                config,
                fragments,
                tx,
                started_at: Instant::now(),
            },
            rx,
        )
    }

    /// Execute the deferred query.
    ///
    /// `full_response` is the complete response from executing the query
    /// with all `@defer` directives stripped. This method splits it into
    /// initial + deferred parts and sends them through the channel.
    pub async fn execute(
        &self,
        full_response: serde_json::Value,
    ) -> Result<(), DeferError> {
        let active_fragments: Vec<_> = self
            .fragments
            .iter()
            .filter(|f| !f.disabled)
            .cloned()
            .collect();

        if active_fragments.is_empty() {
            // No active deferred fragments — send everything as initial
            let initial = InitialPayload {
                data: full_response
                    .get("data")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                errors: extract_errors(&full_response),
                has_next: false,
            };
            self.tx
                .send(DeferredPart::Initial(initial))
                .await
                .map_err(|_| DeferError::ChannelClosed)?;
            return Ok(());
        }

        let data = full_response
            .get("data")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let errors = extract_errors(&full_response);

        // Build the initial payload by removing deferred fields
        let mut initial_data = data.clone();
        let mut deferred_patches: Vec<(DeferredFragment, serde_json::Value)> = Vec::new();

        for fragment in &active_fragments {
            let extracted = extract_and_remove_fields(
                &mut initial_data,
                &fragment.path,
                &fragment.fields,
            );
            deferred_patches.push((fragment.clone(), extracted));
        }

        // Send initial payload
        let initial = InitialPayload {
            data: initial_data,
            errors,
            has_next: true,
        };
        self.tx
            .send(DeferredPart::Initial(initial))
            .await
            .map_err(|_| DeferError::ChannelClosed)?;

        debug!(
            fragment_count = deferred_patches.len(),
            "Sent initial @defer payload, streaming deferred fragments"
        );

        // Send deferred fragments one by one
        let total_patches = deferred_patches.len();
        for (i, (fragment, patch_data)) in deferred_patches.into_iter().enumerate() {
            // Check timeout
            if self.started_at.elapsed() > self.config.total_timeout() {
                warn!("@defer total timeout exceeded, aborting remaining fragments");
                // Send final empty payload
                let final_payload = SubsequentPayload {
                    incremental: vec![],
                    has_next: false,
                };
                let _ = self
                    .tx
                    .send(DeferredPart::Subsequent(final_payload))
                    .await;
                return Err(DeferError::Timeout);
            }

            let is_last = i == total_patches - 1;

            let path: Vec<serde_json::Value> = fragment
                .path
                .iter()
                .map(|p| {
                    // Try to parse as integer for array indices
                    if let Ok(n) = p.parse::<u64>() {
                        serde_json::Value::Number(n.into())
                    } else {
                        serde_json::Value::String(p.clone())
                    }
                })
                .collect();

            let patch = IncrementalPatch {
                data: patch_data,
                path,
                label: fragment.label.clone(),
                errors: Vec::new(),
            };

            let subsequent = SubsequentPayload {
                incremental: vec![patch],
                has_next: !is_last,
            };

            self.tx
                .send(DeferredPart::Subsequent(subsequent))
                .await
                .map_err(|_| DeferError::ChannelClosed)?;

            debug!(
                label = ?fragment.label,
                remaining = total_patches - i - 1,
                "Sent deferred fragment"
            );
        }

        Ok(())
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────

/// Extract the `errors` array from a full GraphQL response JSON.
fn extract_errors(response: &serde_json::Value) -> Vec<serde_json::Value> {
    response
        .get("errors")
        .and_then(|e| e.as_array())
        .cloned()
        .unwrap_or_default()
}

/// Extract specified fields from a JSON object at a given path,
/// removing them from the source and returning the extracted data.
///
/// If `path` is empty, operates on the root object.
fn extract_and_remove_fields(
    data: &mut serde_json::Value,
    path: &[String],
    fields: &[String],
) -> serde_json::Value {
    // Navigate to the target object
    let target = if path.is_empty() {
        data
    } else {
        let mut current = &mut *data;
        for segment in path {
            // Try as array index first
            if let Ok(idx) = segment.parse::<usize>() {
                current = match current {
                    serde_json::Value::Array(arr) => {
                        if idx < arr.len() {
                            &mut arr[idx]
                        } else {
                            return serde_json::Value::Null;
                        }
                    }
                    _ => return serde_json::Value::Null,
                };
            } else {
                current = match current {
                    serde_json::Value::Object(map) => {
                        if let Some(val) = map.get_mut(segment) {
                            val
                        } else {
                            return serde_json::Value::Null;
                        }
                    }
                    _ => return serde_json::Value::Null,
                };
            }
        }
        current
    };

    // Extract fields
    let mut extracted = serde_json::Map::new();
    if let serde_json::Value::Object(map) = target {
        for field in fields {
            if let Some(val) = map.remove(field) {
                extracted.insert(field.clone(), val);
            }
        }
    }

    serde_json::Value::Object(extracted)
}

// ─── Statistics ────────────────────────────────────────────────────────

/// Statistics about `@defer` usage.
#[derive(Debug, Clone, Default)]
pub struct DeferStats {
    /// Total queries that used `@defer`.
    pub total_deferred_queries: u64,
    /// Total deferred fragments resolved.
    pub total_deferred_fragments: u64,
    /// Queries that timed out during deferred resolution.
    pub timeout_count: u64,
    /// Average time to first byte (initial payload) in microseconds.
    pub avg_ttfb_us: u64,
}

// ─── Errors ────────────────────────────────────────────────────────────

/// Errors that can occur during `@defer` processing.
#[derive(Debug, thiserror::Error)]
pub enum DeferError {
    #[error("@defer is not enabled on this gateway")]
    NotEnabled,

    #[error("Too many deferred fragments ({count}/{max})")]
    TooManyFragments { count: usize, max: usize },

    #[error("Deferred fragment resolution timed out")]
    Timeout,

    #[error("Response channel closed")]
    ChannelClosed,

    #[error("Execution failed: {message}")]
    ExecutionFailed { message: String },
}

// ─── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_defer_directive() {
        assert!(has_defer_directive("query { user { ... @defer { email } } }"));
        assert!(has_defer_directive(
            "query { user { ... @defer(label: \"details\") { email } } }"
        ));
        assert!(has_defer_directive("query { user { ... @defer(if: true) { email } } }"));
        assert!(has_defer_directive("query { user { ...@defer { email } } }"));
        assert!(has_defer_directive(
            "query {\n  user {\n    ... @defer {\n      email\n    }\n  }\n}"
        ));

        // Negative cases
        assert!(!has_defer_directive("query { user { name } }"));
        assert!(!has_defer_directive("# @defer\nquery { user { name } }"));
        assert!(!has_defer_directive("query { user { deferred_name } }"));
    }

    #[test]
    fn test_strip_defer_directives() {
        assert_eq!(
            strip_defer_directives("... @defer { email }"),
            "...  { email }"
        );
        assert_eq!(
            strip_defer_directives("... @defer(label: \"details\") { email }"),
            "...  { email }"
        );
        assert_eq!(
            strip_defer_directives("... @defer(label: \"a\", if: true) { email }"),
            "...  { email }"
        );
        // Multiple @defer directives
        let query = "... @defer { a } ... @defer(label: \"b\") { c }";
        let result = strip_defer_directives(query);
        assert!(!result.contains("@defer"));
    }

    #[test]
    fn test_extract_deferred_fragments() {
        let query = r#"query { user { id ... @defer(label: "details") { email address } } }"#;
        let fragments = extract_deferred_fragments(query);
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].label, Some("details".to_string()));
        assert!(fragments[0].fields.contains(&"email".to_string()));
        assert!(fragments[0].fields.contains(&"address".to_string()));
        assert!(!fragments[0].disabled);
    }

    #[test]
    fn test_extract_deferred_fragments_disabled() {
        let query = r#"query { user { ... @defer(if: false) { email } } }"#;
        let fragments = extract_deferred_fragments(query);
        assert_eq!(fragments.len(), 1);
        assert!(fragments[0].disabled);
    }

    #[test]
    fn test_extract_and_remove_fields() {
        let mut data = serde_json::json!({
            "user": {
                "id": "1",
                "name": "Alice",
                "email": "alice@example.com",
                "address": "123 Main St"
            }
        });

        let extracted = extract_and_remove_fields(
            &mut data,
            &["user".to_string()],
            &["email".to_string(), "address".to_string()],
        );

        // The deferred fields should be extracted
        assert_eq!(extracted["email"], "alice@example.com");
        assert_eq!(extracted["address"], "123 Main St");

        // The original data should no longer have those fields
        assert!(data["user"].get("email").is_none());
        assert!(data["user"].get("address").is_none());

        // Non-deferred fields should remain
        assert_eq!(data["user"]["id"], "1");
        assert_eq!(data["user"]["name"], "Alice");
    }

    #[test]
    fn test_extract_and_remove_fields_root() {
        let mut data = serde_json::json!({
            "fast_field": "quick",
            "slow_field": "slow-data"
        });

        let extracted = extract_and_remove_fields(
            &mut data,
            &[],
            &["slow_field".to_string()],
        );

        assert_eq!(extracted["slow_field"], "slow-data");
        assert!(data.get("slow_field").is_none());
        assert_eq!(data["fast_field"], "quick");
    }

    #[test]
    fn test_extract_arg() {
        assert_eq!(
            extract_arg(r#"label: "details", if: true"#, "label"),
            Some("details".to_string())
        );
        assert_eq!(
            extract_arg(r#"label: "details", if: true"#, "if"),
            Some("true".to_string())
        );
        assert_eq!(
            extract_arg(r#"label: "details""#, "missing"),
            None,
        );
    }

    #[test]
    fn test_multipart_formatting() {
        let initial = InitialPayload {
            data: serde_json::json!({"user": {"id": "1"}}),
            errors: vec![],
            has_next: true,
        };

        let part = format_initial_part(&initial, "-");
        assert!(part.contains("--"));
        assert!(part.contains("Content-Type: application/json"));
        assert!(part.contains(r#""hasNext":true"#));

        let subsequent = SubsequentPayload {
            incremental: vec![IncrementalPatch {
                data: serde_json::json!({"email": "a@b.com"}),
                path: vec![serde_json::Value::String("user".to_string())],
                label: Some("details".to_string()),
                errors: vec![],
            }],
            has_next: false,
        };

        let part = format_subsequent_part(&subsequent, "-");
        assert!(part.contains("---")); // closing boundary
        assert!(part.contains(r#""hasNext":false"#));
    }

    #[test]
    fn test_config_presets() {
        let prod = DeferConfig::production();
        assert_eq!(prod.max_deferred_fragments, 8);
        assert!(prod.enabled);

        let dev = DeferConfig::development();
        assert_eq!(dev.max_deferred_fragments, 64);

        let disabled = DeferConfig::disabled();
        assert!(!disabled.enabled);
    }

    #[tokio::test]
    async fn test_deferred_execution_no_fragments() {
        let config = DeferConfig::default();
        let fragments = vec![];
        let (exec, mut rx) = DeferredExecution::new(config, fragments);

        let response = serde_json::json!({
            "data": {"user": {"id": "1", "name": "Alice"}}
        });

        exec.execute(response).await.unwrap();

        match rx.recv().await.unwrap() {
            DeferredPart::Initial(payload) => {
                assert!(!payload.has_next);
                assert_eq!(payload.data["user"]["name"], "Alice");
            }
            _ => panic!("Expected initial payload"),
        }
    }

    #[tokio::test]
    async fn test_deferred_execution_with_fragments() {
        let config = DeferConfig::default();
        let fragments = vec![DeferredFragment {
            label: Some("details".to_string()),
            path: vec!["user".to_string()],
            fields: vec!["email".to_string()],
            disabled: false,
        }];
        let (exec, mut rx) = DeferredExecution::new(config, fragments);

        let response = serde_json::json!({
            "data": {"user": {"id": "1", "name": "Alice", "email": "alice@example.com"}}
        });

        exec.execute(response).await.unwrap();

        // Check initial payload
        match rx.recv().await.unwrap() {
            DeferredPart::Initial(payload) => {
                assert!(payload.has_next);
                // email should be removed from initial payload
                assert!(payload.data["user"].get("email").is_none());
                assert_eq!(payload.data["user"]["id"], "1");
            }
            _ => panic!("Expected initial payload"),
        }

        // Check deferred patch
        match rx.recv().await.unwrap() {
            DeferredPart::Subsequent(payload) => {
                assert!(!payload.has_next);
                assert_eq!(payload.incremental.len(), 1);
                assert_eq!(
                    payload.incremental[0].data["email"],
                    "alice@example.com"
                );
                assert_eq!(
                    payload.incremental[0].label,
                    Some("details".to_string())
                );
            }
            _ => panic!("Expected subsequent payload"),
        }
    }

    #[tokio::test]
    async fn test_deferred_execution_disabled_fragment() {
        let config = DeferConfig::default();
        let fragments = vec![DeferredFragment {
            label: Some("details".to_string()),
            path: vec!["user".to_string()],
            fields: vec!["email".to_string()],
            disabled: true, // disabled via @defer(if: false)
        }];
        let (exec, mut rx) = DeferredExecution::new(config, fragments);

        let response = serde_json::json!({
            "data": {"user": {"id": "1", "email": "alice@example.com"}}
        });

        exec.execute(response).await.unwrap();

        // Since fragment is disabled, everything should be in initial
        match rx.recv().await.unwrap() {
            DeferredPart::Initial(payload) => {
                assert!(!payload.has_next);
                assert_eq!(payload.data["user"]["email"], "alice@example.com");
            }
            _ => panic!("Expected initial payload"),
        }
    }
}
