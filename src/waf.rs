//! Web Application Firewall (WAF) Middleware
//!
//! Provides SQL Injection (SQLi), XSS, and NoSQL Injection detection and prevention by inspecting
//! GraphQL variables and arguments for common malicious patterns.

use crate::error::{Error, Result};
use crate::middleware::{Context, Middleware};
use regex::Regex;
use std::sync::OnceLock;

/// Common SQL injection patterns
/// 
/// Ref: OWASP SQL Injection Prevention Cheat Sheet
/// Common SQL injection patterns
/// 
/// Ref: OWASP SQL Injection Prevention Cheat Sheet & payload lists
const SQLI_PATTERNS: &[&str] = &[
    // Union-based
    r"(?i)union\s+(all\s+)?select",
    r"(?i)union\s+distinct\s+select",
    // Error-based / Stacked queries / Blind
    r"(?i);\s*drop\s+table",
    r"(?i);\s*insert\s+into",
    r"(?i);\s*update\s+.*\s+set",
    r"(?i);\s*delete\s+from",
    r"(?i);\s*alter\s+table",
    r"(?i);\s*create\s+table",
    r"(?i);\s*truncate\s+table",
    r"(?i);\s*merge\s+into",
    r"(?i);\s*declare\s+",
    r"(?i);\s*shutdown",
    r"(?i)waitfor\s+delay",
    r"(?i)pg_sleep\(",
    r"(?i)benchmark\(",
    r"(?i)sleep\(",
    r"(?i)exec(\s|\()",
    r"(?i)execute\s+immediate",
    r"(?i)into\s+outfile",
    r"(?i)into\s+dumpfile",
    r"(?i)load_file\(",
    r"(?i)information_schema",
    r"(?i)sys\.user_objects",
    r"(?i)sys\.objects",
    r"(?i)xp_cmdshell",
    // Boolean-based (blind)
    r"(?i)or\s+1\s*=\s*1",
    r"(?i)or\s+'1'\s*=\s*'1'",
    r"(?i)'\s+or\s+'",
    r"(?i)or\s+true\s*--",
    r"(?i)'\s+or\s+true",
    r#"(?i)"\s+or\s+true"#,
    r"(?i)and\s+1\s*=\s*1",
    r"(?i)\)\s*or\s+\(",
    // Comment injection
    r"--",
    r"(?s)/\*.*\*/", // (?s) enables dot matches newline
    r"(?i)#\s",      // MySQL comment
    r"(?i);\s*--",
    // Hex encoded patterns often used in SQLi
    r"0x[0-9a-fA-F]+",
    // Functions
    r"(?i)version\(\)",
    r"(?i)database\(\)",
    r"(?i)user\(\)",
    r"(?i)current_user",
    r"(?i)system_user",
    r"(?i)session_user",
    r"(?i)@@version",
    r"(?i)char\(\d+\)",
    r"(?i)ascii\(",
    r"(?i)substring\(",
];

/// Common XSS patterns
///
/// Ref: OWASP XSS Prevention Cheat Sheet
const XSS_PATTERNS: &[&str] = &[
    r"(?i)<script.*?>.*?</script>",
    r"(?i)<script",
    r"(?i)</script>",
    r"(?i)javascript:",
    r"(?i)vbscript:",
    r"(?i)livescript:",
    r"(?i)data:text/html",
    r"(?i)data:text/xml",
    r"(?i)data:image/svg\+xml",
    // Event handlers
    r"(?i)on(load|error|click|mouseover|mouseout|focus|blur|change|submit|dblclick|keydown|keypress|keyup|mousedown|mouseup|contextmenu|abort|afterprint|animationend|animationiteration|animationstart|beforeprint|beforeunload|canplay|canplaythrough|drag|dragend|dragenter|dragleave|dragover|dragstart|drop|durationchange|ended|fullscreenchange|fullscreenerror|input|invalid|loadeddata|loadedmetadata|loadstart|message|offline|online|open|pagehide|pageshow|pause|play|playing|popstate|progress|ratechange|reset|resize|scroll|seeked|seeking|select|show|stalled|storage|suspend|timeupdate|toggle|touchcancel|touchend|touchmove|touchstart|transitionend|unload|volumechange|waiting|wheel)\s*=",
    // Dangerous tags
    r"(?i)<(iframe|object|embed|svg|applet|meta|link|style|form|input|button|base|body|head|html|marquee|blink|frameset|video|audio|details|dialog|menu|picture|source|track|canvas|map|area)",
    r"(?i)</(iframe|object|embed|svg|applet|meta|link|style|form|input|button|base|body|head|html|marquee|blink|frameset|video|audio|details|dialog|menu|picture|source|track|canvas|map|area)",
    // Attributes
    r#"(?i)href\s*=\s*['"]?javascript:"#,
    r#"(?i)src\s*=\s*['"]?javascript:"#,
    r#"(?i)xlink:href"#,
    r#"(?i)fscommand"#,
    r#"(?i)seeksegmenttime"#,
    r#"(?i)br"#, // often used to break out of attributes
    r#"(?i)style\s*=\s*['"].*expression\("#, // IE expression
    r#"(?i)style\s*=\s*['"].*url\("#,
];

/// Common NoSQL injection patterns (MongoDB focus)
const NOSQLI_PATTERNS: &[&str] = &[
    r"(?i)\$where",
    r"(?i)\$ne",
    r"(?i)\$gt",
    r"(?i)\$gte",
    r"(?i)\$lt",
    r"(?i)\$lte",
    r"(?i)\$in",
    r"(?i)\$nin",
    r"(?i)\$regex",
    r"(?i)\$exists",
    r"(?i)\$or",
    r"(?i)\$and",
    r"(?i)\$not",
    r"(?i)\$nor",
    r"(?i)\$jsonSchema",
    r"(?i)\$expr",
    r"(?i)\$mod",
    r"(?i)\$all",
    r"(?i)\$size",
    r"(?i)\$type",
    r"(?i)\$elemMatch",
    r"(?i)\$slice",
    r"(?i)\$text",
    r"(?i)\$search",
    r"(?i)new\s+Date\(", // Server side JS execution
    r"(?i)this\.",
    r"(?i)function\(",
];

/// Command Injection patterns (OS Command Injection)
///
/// NOTE: We intentionally exclude `\n` (newline) and `$` from the separator
/// group because they cause false positives in GraphQL queries:
///   - `\n` is normal multi-line formatting (e.g. `{\n  id\n  message\n}`)
///   - `$` is the GraphQL variable prefix (e.g. `$id`, `$name`)
///     We also use word boundary `\b` after the command name to avoid partial
///     matches (e.g. "identity" should not trigger on "id").
const CMDI_PATTERNS: &[&str] = &[
    // Separators with commands — only real shell metacharacters
    r"(?i)(;|\||\|\||&|&&|`)\s*\b(ls|cat|rm|mv|cp|echo|wget|curl|ping|nc|netcat|nmap|whoami|id|pwd|grep|awk|sed|tar|zip|unzip|python|perl|ruby|gcc|make|kill|sudo|su|ssh|scp|ftp|telnet|dig|nslookup|ifconfig|ip|route|ps|top|free|df|du|uname|hostname|env|export|alias|declare|mount|umount|chmod|chown|chgrp|touch|mkdir|rmdir)\b",
    // Specific constructs
    r"(?i)\$(?:\(|`)[^`)]+(?:\)|`)", // $(...) or `...`
    r"(?i)/bin/sh",
    r"(?i)/bin/bash",
    r"(?i)/bin/zsh",
    r"(?i)/usr/bin/",
    r"(?i)cmd\.exe",
    r"(?i)powershell",
    r"(?i)pwsh",
    r"(?i)bash\s+-i",
    r"(?i)sh\s+-i",
];

/// Path Traversal patterns (LFI/RFI)
const TRAVERSAL_PATTERNS: &[&str] = &[
    r"\.\./",
    r"\.\.\\",
    r"\.\.%2f",
    r"\.\.%5c",
    r"%2e%2e%2f",
    r"%2e%2e/",
    r"\.\./\.\./",
    r"(?i)/etc/passwd",
    r"(?i)/etc/shadow",
    r"(?i)/etc/group",
    r"(?i)/etc/hosts",
    r"(?i)/etc/issue",
    r"(?i)/proc/self/environ",
    r"(?i)/proc/self/cmdline",
    r"(?i)c:\\windows",
    r"(?i)c:\\winnt",
    r"(?i)boot\.ini",
    r"(?i)system32",
    r"(?i)\\windows\\",
];

/// LDAP Injection patterns
const LDAP_PATTERNS: &[&str] = &[
    r"\*\(",
    r"\)\*",
    r"\(&",
    r"\|&",
    r"\(!",
    r"\)\(",
    r"user\s*=\s*\*",
    r"admin\s*=\s*\*",
];

/// Server Side Template Injection (SSTI) patterns
const SSTI_PATTERNS: &[&str] = &[
    r"\{\{.*\}\}", // Moustache/Handlebars/Jinja/etc
    r"\$\{.*\}",   // EL
    r"<%=",       // ERB/JSP
    r"#\{.*\}",   // Ruby
    r"\*\{.*\}",
    r"\[\[.*\]\]", // Flask/Jinja2 alternative
    r"\{\%.*\%\}",
];

/// Compiled regex for SQLi detection
fn sqli_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        let pattern = SQLI_PATTERNS.join("|");
        Regex::new(&pattern).expect("Invalid SQLi regex pattern")
    })
}

/// Compiled regex for XSS detection
fn xss_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        let pattern = XSS_PATTERNS.join("|");
        Regex::new(&pattern).expect("Invalid XSS regex pattern")
    })
}

/// Compiled regex for NoSQLi detection
fn nosqli_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        let pattern = NOSQLI_PATTERNS.join("|");
        Regex::new(&pattern).expect("Invalid NoSQLi regex pattern")
    })
}

/// Compiled regex for CMDI detection
fn cmdi_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        let pattern = CMDI_PATTERNS.join("|");
        Regex::new(&pattern).expect("Invalid CMDI regex pattern")
    })
}

/// Compiled regex for Traversal detection
fn traversal_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        let pattern = TRAVERSAL_PATTERNS.join("|");
        Regex::new(&pattern).expect("Invalid Traversal regex pattern")
    })
}

/// Compiled regex for LDAP injection detection
fn ldap_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        let pattern = LDAP_PATTERNS.join("|");
        Regex::new(&pattern).expect("Invalid LDAP regex pattern")
    })
}

/// Compiled regex for SSTI detection
fn ssti_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        let pattern = SSTI_PATTERNS.join("|");
        Regex::new(&pattern).expect("Invalid SSTI regex pattern")
    })
}

/// WAF Configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WafConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub block_sqli: bool,
    #[serde(default = "default_true")]
    pub block_xss: bool,
    #[serde(default = "default_true")]
    pub block_nosqli: bool,
    #[serde(default = "default_true")]
    pub block_cmdi: bool,
    #[serde(default = "default_true")]
    pub block_traversal: bool,
    #[serde(default = "default_true")]
    pub block_ldap: bool,
    #[serde(default = "default_true")]
    pub block_ssti: bool,
    /// Optional custom regex patterns supplied by the user.
    /// Each pattern is treated as an independent rule; they are OR‑combined.
    #[serde(default)]
    pub custom_patterns: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for WafConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            block_sqli: true,
            block_xss: true,
            block_nosqli: true,
            block_cmdi: true,
            block_traversal: true,
            block_ldap: true,
            block_ssti: true,
            custom_patterns: Vec::new(),
        }
    }
}

/// WAF Middleware
/// 
/// Intercepts requests and validates input variables against malicious patterns.
pub struct WafMiddleware {
    pub config: WafConfig,
}

impl WafMiddleware {
    pub fn new(config: WafConfig) -> Self {
        Self { config }
    }
}

impl Default for WafMiddleware {
    fn default() -> Self {
        Self::new(WafConfig::default())
    }
}

/// Recursively inspect JSON values for malicious patterns
pub fn validate_json(value: &serde_json::Value) -> Result<()> {
    // Default to full protection when called directly
    validate_json_with_config(value, &WafConfig::default())
}

pub fn validate_json_with_config(value: &serde_json::Value, config: &WafConfig) -> Result<()> {
    if !config.enabled {
        return Ok(());
    }

    if config.block_sqli {
        if let Some(matched) = check_pattern(value, sqli_regex()) {
            tracing::warn!(match_val = %matched, "SQL Injection attempt detected");
            return Err(Error::Validation(format!("Potential SQL Injection detected: {}", matched)));
        }
    }

    if config.block_xss {
        if let Some(matched) = check_pattern(value, xss_regex()) {
            tracing::warn!(match_val = %matched, "XSS attempt detected");
            return Err(Error::Validation(format!("Potential XSS detected: {}", matched)));
        }
    }

    if config.block_nosqli {
         // NoSQLi often uses special keys in JSON objects (e.g. {"$ne": ...})
         // We need a specific check for keys as well for strict NoSQLi protection
         if let Some(matched) = check_keys(value, nosqli_regex()) {
             tracing::warn!(match_val = %matched, "NoSQL Injection attempt detected in keys");
             return Err(Error::Validation(format!("Potential NoSQL Injection detected: {}", matched)));
         }
         // Also check values
         if let Some(matched) = check_pattern(value, nosqli_regex()) {
            tracing::warn!(match_val = %matched, "NoSQL Injection attempt detected");
            return Err(Error::Validation(format!("Potential NoSQL Injection detected: {}", matched)));
         }
    }

    if config.block_cmdi {
        if let Some(matched) = check_pattern(value, cmdi_regex()) {
            tracing::warn!(match_val = %matched, "Command Injection attempt detected");
            return Err(Error::Validation(format!("Potential Command Injection detected: {}", matched)));
        }
    }

    if config.block_traversal {
        if let Some(matched) = check_pattern(value, traversal_regex()) {
            tracing::warn!(match_val = %matched, "Path Traversal attempt detected");
            return Err(Error::Validation(format!("Potential Path Traversal detected: {}", matched)));
        }
    }

    if config.block_ldap {
        if let Some(matched) = check_pattern(value, ldap_regex()) {
            tracing::warn!(match_val = %matched, "LDAP Injection attempt detected");
            return Err(Error::Validation(format!("Potential LDAP Injection detected: {}", matched)));
        }
    }

    if config.block_ssti {
        if let Some(matched) = check_pattern(value, ssti_regex()) {
            tracing::warn!(match_val = %matched, "SSTI attempt detected");
            return Err(Error::Validation(format!("Potential SSTI detected: {}", matched)));
        }
    }

    // Custom user‑provided patterns (applied to string values only)
    if !config.custom_patterns.is_empty() {
        // Only check string values; other JSON types are ignored for custom rules.
        if let serde_json::Value::String(s) = value {
            if let Some(pat) = check_custom(s, &config.custom_patterns) {
                tracing::warn!(match_val = %pat, "Custom WAF pattern matched");
                return Err(Error::Validation(format!("Custom WAF rule triggered: {}", pat)));
            }
        }
    }

    Ok(())
}

/// Recursively inspect JSON values for pattern matches
fn check_pattern(value: &serde_json::Value, regex: &Regex) -> Option<String> {
    match value {
        serde_json::Value::String(s) => {
            if regex.is_match(s) {
                return Some(s.clone());
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                if let Some(s) = check_pattern(v, regex) {
                    return Some(s);
                }
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values() {
                if let Some(s) = check_pattern(v, regex) {
                    return Some(s);
                }
            }
        }
        _ => {}
    }
    None
}

/// Recursively inspect JSON keys for pattern matches (important for NoSQLi)
fn check_keys(value: &serde_json::Value, regex: &Regex) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if regex.is_match(k) {
                    return Some(k.clone());
                }
                if let Some(s) = check_keys(v, regex) {
                    return Some(s);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                 if let Some(s) = check_keys(v, regex) {
                    return Some(s);
                }
            }
        }
        _ => {}
    }
    None
}

/// Helper to evaluate user‑provided custom patterns.
/// Returns the first matching pattern string, if any.
fn check_custom(value: &str, patterns: &[String]) -> Option<String> {
    for pat in patterns {
        if let Ok(re) = Regex::new(pat) {
            if re.is_match(value) {
                return Some(pat.clone());
            }
        }
    }
    None
}

#[async_trait::async_trait]
impl Middleware for WafMiddleware {
    async fn call(&self, ctx: &mut Context) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        for (name, value) in &ctx.headers {
            if let Ok(value_str) = value.to_str() {
                if self.config.block_sqli && sqli_regex().is_match(value_str) {
                    tracing::warn!(header = ?name, match_val = value_str, "SQL Injection attempt detected in headers");
                    return Err(Error::Validation(format!("Potential SQL Injection detected in header: {}", name)));
                }
                if self.config.block_xss && xss_regex().is_match(value_str) {
                    tracing::warn!(header = ?name, match_val = value_str, "XSS attempt detected in headers");
                    return Err(Error::Validation(format!("Potential XSS detected in header: {}", name)));
                }
                if self.config.block_nosqli && nosqli_regex().is_match(value_str) {
                    tracing::warn!(header = ?name, match_val = value_str, "NoSQL Injection attempt detected in headers");
                     return Err(Error::Validation(format!("Potential NoSQL Injection detected in header: {}", name)));
                }
                if self.config.block_cmdi && cmdi_regex().is_match(value_str) {
                    tracing::warn!(header = ?name, match_val = value_str, "Command Injection attempt detected in headers");
                     return Err(Error::Validation(format!("Potential Command Injection detected in header: {}", name)));
                }
                if self.config.block_traversal && traversal_regex().is_match(value_str) {
                    tracing::warn!(header = ?name, match_val = value_str, "Path Traversal attempt detected in headers");
                     return Err(Error::Validation(format!("Potential Path Traversal detected in header: {}", name)));
                }
                if self.config.block_ldap && ldap_regex().is_match(value_str) {
                    tracing::warn!(header = ?name, match_val = value_str, "LDAP Injection attempt detected in headers");
                     return Err(Error::Validation(format!("Potential LDAP Injection detected in header: {}", name)));
                }
                if self.config.block_ssti && ssti_regex().is_match(value_str) {
                    tracing::warn!(header = ?name, match_val = value_str, "SSTI attempt detected in headers");
                     return Err(Error::Validation(format!("Potential SSTI detected in header: {}", name)));
                }
                // Custom patterns on header values
                if !self.config.custom_patterns.is_empty() {
                    if let Some(pat) = check_custom(value_str, &self.config.custom_patterns) {
                        tracing::warn!(header = ?name, match_val = pat, "Custom WAF pattern matched in header");
                        return Err(Error::Validation(format!("Custom WAF rule triggered in header {}: {}", name, pat)));
                    }
                }
            }
        }        
        Ok(())
    }

    fn name(&self) -> &'static str {
        "WafMiddleware"
    }
}

/// Validate GraphQL request against WAF rules using default config
pub fn validate_request(req: &async_graphql::Request) -> Result<()> {
    // Check variables
    let vars = serde_json::to_value(&req.variables).unwrap_or(serde_json::Value::Null);
    validate_json(&vars)?;

    // Check query string for basic signatures
    if sqli_regex().is_match(&req.query) {
         tracing::warn!("SQL Injection attempt detected in query string");
         return Err(Error::Validation("Potential SQL Injection detected".to_string()));
    }
    // XSS in query string?
    if xss_regex().is_match(&req.query) {
        tracing::warn!("XSS attempt detected in query string");
        return Err(Error::Validation("Potential XSS detected".to_string()));
    }
    // NoSQLi in query string? less likely in standard GQL syntax but possible in directives
    if nosqli_regex().is_match(&req.query) {
        tracing::warn!("NoSQL Injection attempt detected in query string");
        return Err(Error::Validation("Potential NoSQL Injection detected".to_string()));
    }
    // Command Injection in query string?
    if cmdi_regex().is_match(&req.query) {
        tracing::warn!("Command Injection attempt detected in query string");
        return Err(Error::Validation("Potential Command Injection detected".to_string()));
    }
    // Path Traversal in query string?
    if traversal_regex().is_match(&req.query) {
        tracing::warn!("Path Traversal attempt detected in query string");
        return Err(Error::Validation("Potential Path Traversal detected".to_string()));
    }
    // LDAP Injection in query string?
    if ldap_regex().is_match(&req.query) {
        tracing::warn!("LDAP Injection attempt detected in query string");
        return Err(Error::Validation("Potential LDAP Injection detected".to_string()));
    }
    // SSTI in query string?
    if ssti_regex().is_match(&req.query) {
        tracing::warn!("SSTI attempt detected in query string");
        return Err(Error::Validation("Potential SSTI detected".to_string()));
    }

    Ok(())
}

/// Validate raw query and variables against WAF rules
pub fn validate_raw(query: &str, variables: Option<&serde_json::Value>, config: &WafConfig) -> Result<()> {
    if !config.enabled {
        return Ok(());
    }

    // Check query string
    if config.block_sqli && sqli_regex().is_match(query) {
        tracing::warn!("SQL Injection attempt detected in query string");
        return Err(Error::Validation("Potential SQL Injection detected".to_string()));
    }
    if config.block_xss && xss_regex().is_match(query) {
        tracing::warn!("XSS attempt detected in query string");
        return Err(Error::Validation("Potential XSS detected".to_string()));
    }
    if config.block_nosqli && nosqli_regex().is_match(query) {
        tracing::warn!("NoSQL Injection attempt detected in query string");
         return Err(Error::Validation("Potential NoSQL Injection detected".to_string()));
    }
    if config.block_cmdi && cmdi_regex().is_match(query) {
        tracing::warn!("Command Injection attempt detected in query string");
         return Err(Error::Validation("Potential Command Injection detected".to_string()));
    }
    if config.block_traversal && traversal_regex().is_match(query) {
        tracing::warn!("Path Traversal attempt detected in query string");
         return Err(Error::Validation("Potential Path Traversal detected".to_string()));
    }
    if config.block_ldap && ldap_regex().is_match(query) {
        tracing::warn!("LDAP Injection attempt detected in query string");
         return Err(Error::Validation("Potential LDAP Injection detected".to_string()));
    }
    if config.block_ssti && ssti_regex().is_match(query) {
        tracing::warn!("SSTI attempt detected in query string");
         return Err(Error::Validation("Potential SSTI detected".to_string()));
    }

    // Check variables
    if let Some(vars) = variables {
        validate_json_with_config(vars, config)?;
    }

    Ok(())
}

/// Check for introspection queries
pub fn is_introspection(query: &str) -> bool {
    query.contains("__schema") || query.contains("__type")
}

/// Validate HTTP headers against WAF rules
pub fn validate_headers(headers: &http::HeaderMap, config: &WafConfig) -> Result<()> {
    if !config.enabled {
        return Ok(());
    }

    for (name, value) in headers {
        if let Ok(value_str) = value.to_str() {
            if config.block_sqli && sqli_regex().is_match(value_str) {
                tracing::warn!(header = ?name, match_val = value_str, "SQL Injection attempt detected in headers");
                return Err(Error::Validation(format!("Potential SQL Injection detected in header: {:?}", name)));
            }
            if config.block_xss && xss_regex().is_match(value_str) {
                tracing::warn!(header = ?name, match_val = value_str, "XSS attempt detected in headers");
                return Err(Error::Validation(format!("Potential XSS detected in header: {:?}", name)));
            }
            if config.block_nosqli && nosqli_regex().is_match(value_str) {
                tracing::warn!(header = ?name, match_val = value_str, "NoSQL Injection attempt detected in headers");
                return Err(Error::Validation(format!("Potential NoSQL Injection detected in header: {:?}", name)));
            }
            if config.block_cmdi && cmdi_regex().is_match(value_str) {
                tracing::warn!(header = ?name, match_val = value_str, "Command Injection attempt detected in headers");
                return Err(Error::Validation(format!("Potential Command Injection detected in header: {:?}", name)));
            }
            if config.block_traversal && traversal_regex().is_match(value_str) {
                tracing::warn!(header = ?name, match_val = value_str, "Path Traversal attempt detected in headers");
                return Err(Error::Validation(format!("Potential Path Traversal detected in header: {:?}", name)));
            }
            if config.block_ldap && ldap_regex().is_match(value_str) {
                tracing::warn!(header = ?name, match_val = value_str, "LDAP Injection attempt detected in headers");
                return Err(Error::Validation(format!("Potential LDAP Injection detected in header: {:?}", name)));
            }
            if config.block_ssti && ssti_regex().is_match(value_str) {
                tracing::warn!(header = ?name, match_val = value_str, "SSTI attempt detected in headers");
                return Err(Error::Validation(format!("Potential SSTI detected in header: {:?}", name)));
            }
        }
    }
    Ok(())
}

/// Validate a query string specifically
pub fn validate_query_string(query: &str, config: &WafConfig) -> Result<()> {
    if !config.enabled {
        return Ok(());
    }
    
    if config.block_sqli && sqli_regex().is_match(query) {
        tracing::warn!("SQL Injection attempt detected in query string");
        return Err(Error::Validation("Potential SQL Injection detected".to_string()));
    }
    if config.block_xss && xss_regex().is_match(query) {
        tracing::warn!("XSS attempt detected in query string");
        return Err(Error::Validation("Potential XSS detected".to_string()));
    }
    if config.block_nosqli && nosqli_regex().is_match(query) {
        tracing::warn!("NoSQL Injection attempt detected in query string");
        return Err(Error::Validation("Potential NoSQL Injection detected".to_string()));
    }
    if config.block_cmdi && cmdi_regex().is_match(query) {
        tracing::warn!("Command Injection attempt detected in query string");
        return Err(Error::Validation("Potential Command Injection detected".to_string()));
    }
    if config.block_traversal && traversal_regex().is_match(query) {
        tracing::warn!("Path Traversal attempt detected in query string");
        return Err(Error::Validation("Potential Path Traversal detected".to_string()));
    }
    if config.block_ldap && ldap_regex().is_match(query) {
        tracing::warn!("LDAP Injection attempt detected in query string");
        return Err(Error::Validation("Potential LDAP Injection detected".to_string()));
    }
    if config.block_ssti && ssti_regex().is_match(query) {
        tracing::warn!("SSTI attempt detected in query string");
        return Err(Error::Validation("Potential SSTI detected".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for https://github.com/Protocol-Lattice/grpc_graphql_gateway/issues/67
    /// `id` (and other short command names) must NOT trigger CMDI detection
    /// when used as normal GraphQL field names.
    #[test]
    fn test_cmdi_no_false_positive_on_id_field() {
        let queries = vec![
            // Inline query with `id`
            r#"query Test { hello(name: "rawr") { id message } }"#,
            // Multi-line query (the original failing case)
            "query Test {\n  hello(name: \"rawr\") {\n    id\n    message\n  }\n}",
            // `id` as an argument
            r#"{ node(id: "123") { ... on User { id name } } }"#,
            // `id` as a GraphQL variable
            r#"query($id: ID!) { user(id: $id) { id name } }"#,
            // Multiple short command-name-like fields
            "{ system { id ip hostname env } }",
            // Entity with `id` key
            r#"{ _entities(representations: [{__typename: "User", id: "1"}]) { ... on User { id } } }"#,
        ];

        let config = WafConfig::default();
        for query in &queries {
            let result = validate_raw(query, None, &config);
            assert!(
                result.is_ok(),
                "False positive CMDI on query: {}",
                query.replace('\n', "\\n")
            );
        }
    }

    /// Verify that real command injection attempts are still caught.
    #[test]
    fn test_cmdi_catches_real_attacks() {
        let attacks = vec![
            "; whoami",
            "| cat /etc/passwd",
            "&& rm -rf /",
            "|| id",
            "; ls",
            "` whoami `",
            "& curl http://evil.com",
            "/bin/sh",
            "/bin/bash",
            "powershell",
            "$(whoami)",
        ];

        let config = WafConfig::default();
        for attack in &attacks {
            let result = validate_raw(attack, None, &config);
            assert!(
                result.is_err(),
                "Missed real CMDI attack: {}",
                attack
            );
        }
    }

    /// Verify that `id`-like words inside longer identifiers don't trigger.
    #[test]
    fn test_cmdi_no_match_on_partial_words() {
        let queries = vec![
            "{ user { identity provider } }",
            "{ widget { uuid } }",
            "{ order { productId } }",
            "{ config { endpoint } }",
        ];

        let config = WafConfig::default();
        for query in &queries {
            let result = validate_raw(query, None, &config);
            assert!(
                result.is_ok(),
                "False positive CMDI on partial word in query: {}",
                query
            );
        }
    }
}
