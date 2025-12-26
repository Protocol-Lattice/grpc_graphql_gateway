//! Web Application Firewall (WAF) Middleware
//!
//! Provides SQL Injection (SQLi) detection and prevention by inspecting
//! GraphQL variables and arguments for common malicious patterns.

use crate::error::{Error, Result};
use crate::middleware::{Context, Middleware};
use regex::Regex;
use std::sync::OnceLock;

/// Common SQL injection patterns
/// 
/// Ref: OWASP SQL Injection Prevention Cheat Sheet
const SQLI_PATTERNS: &[&str] = &[
    // Union-based
    r"(?i)union\s+(all\s+)?select",
    // Error-based / Stacked queries
    r"(?i);\s*drop\s+table",
    r"(?i);\s*insert\s+into",
    r"(?i);\s*update\s+",
    r"(?i);\s*delete\s+from",
    r"(?i);\s*shutdown",
    // Boolean-based (blind)
    r"(?i)or\s+1\s*=\s*1",
    r"(?i)or\s+'1'\s*=\s*'1'",
    r"(?i)'\s+or\s+'",
    // Comment injection
    r"--",
    r"/\*.*\*/",
    // Hex encoded patterns often used in SQLi
    r"0x[0-9a-fA-F]+",
];

/// Compiled regex for SQLi detection
fn sqli_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        let pattern = SQLI_PATTERNS.join("|");
        Regex::new(&pattern).expect("Invalid SQLi regex pattern")
    })
}

/// WAF Middleware
/// 
/// Intercepts requests and validates input variables against SQLi patterns.
pub struct WafMiddleware {
    /// Strict mode (block request) vs Report only (log warning)
    pub strict: bool,
}

impl WafMiddleware {
    pub fn new(strict: bool) -> Self {
        Self { strict }
    }
}

impl Default for WafMiddleware {
    fn default() -> Self {
        Self::new(true)
    }
}

/// Recursively inspect JSON values for SQLi patterns
pub fn validate_json(value: &serde_json::Value) -> Result<()> {
    if let Some(matched) = has_sqli(value) {
        tracing::warn!(match_val = %matched, "SQL Injection attempt detected");
        return Err(Error::Validation(format!("Potential SQL Injection detected: {}", matched)));
    }
    Ok(())
}

/// Recursively inspect JSON values for SQLi patterns
fn has_sqli(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => {
            if sqli_regex().is_match(s) {
                return Some(s.clone());
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                if let Some(s) = has_sqli(v) {
                    return Some(s);
                }
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values() {
                if let Some(s) = has_sqli(v) {
                    return Some(s);
                }
            }
        }
        _ => {}
    }
    None
}

#[async_trait::async_trait]
impl Middleware for WafMiddleware {
    async fn call(&self, ctx: &mut Context) -> Result<()> {
        // WAF validation is primarily handled in the runtime's handle_http method,
        // (checking body variables), but we can also check headers here for 
        // SQLi patterns (e.g. User-Agent, X-Headers injections).
        
        for (name, value) in &ctx.headers {
            if let Ok(value_str) = value.to_str() {
                if sqli_regex().is_match(value_str) {
                    tracing::warn!(
                        header = ?name,
                        match_val = value_str,
                        "SQL Injection attempt detected in headers"
                    );
                    return Err(Error::Validation(format!(
                        "Potential SQL Injection detected in header: {}", 
                        name
                    )));
                }
            }
        }
        
        Ok(())
    }

    fn name(&self) -> &'static str {
        "WafMiddleware"
    }
}

/// Validate GraphQL request against WAF rules
pub fn validate_request(req: &async_graphql::Request) -> Result<()> {
    // Check variables
    let vars = serde_json::to_value(&req.variables).unwrap_or(serde_json::Value::Null);
    if let Some(matched) = has_sqli(&vars) {
        tracing::warn!(match_val = %matched, "SQL Injection attempt detected in variables");
        return Err(Error::Validation("Potential SQL Injection detected".to_string()));
    }

    // Check query string (basic check for obvious patterns in raw query)
    // Note: This is prone to false positives if not careful, but useful for 
    // blocking 'OR 1=1' style attacks in the query body itself.
    if sqli_regex().is_match(&req.query) {
         tracing::warn!("SQL Injection attempt detected in query string");
         return Err(Error::Validation("Potential SQL Injection detected".to_string()));
    }

    Ok(())
}
