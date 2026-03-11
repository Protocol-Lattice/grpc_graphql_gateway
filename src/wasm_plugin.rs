//! WebAssembly (WASM) Plugin Sandboxing
//!
//! This module provides a WASM-based plugin system that allows users to write
//! custom gateway logic (routing, authentication, request/response transformation)
//! in any language that compiles to WASM (Rust, Go, AssemblyScript, C/C++, etc.).
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────┐
//! │                        Gateway Process                          │
//! │                                                                  │
//! │  ┌──────────────┐   ┌──────────────┐   ┌──────────────┐        │
//! │  │  WASM Plugin  │   │  WASM Plugin  │   │  WASM Plugin  │        │
//! │  │  (Rust .wasm) │   │  (Go .wasm)   │   │  (AS .wasm)   │        │
//! │  │              │   │              │   │              │        │
//! │  │ ┌──────────┐ │   │ ┌──────────┐ │   │ ┌──────────┐ │        │
//! │  │ │  Memory   │ │   │ │  Memory   │ │   │ │  Memory   │ │        │
//! │  │ │  (sandboxed)│ │   │ │(sandboxed)│ │   │ │(sandboxed)│ │        │
//! │  │ └──────────┘ │   │ └──────────┘ │   │ └──────────┘ │        │
//! │  └──────┬───────┘   └──────┬───────┘   └──────┬───────┘        │
//! │         │                  │                  │                │
//! │         └──────────────────┼──────────────────┘                │
//! │                            │                                    │
//! │                    ┌───────▼───────┐                            │
//! │                    │  Host ABI     │                            │
//! │                    │  (log, headers│                            │
//! │                    │   metadata)   │                            │
//! │                    └───────────────┘                            │
//! └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Security
//!
//! - **Memory Isolation**: Each WASM module runs in its own linear memory space.
//! - **Fuel Metering**: CPU time is bounded via wasmtime's fuel mechanism.
//! - **Memory Limits**: Configurable maximum memory per plugin instance.
//! - **No System Access**: WASM modules cannot access the filesystem, network,
//!   or any OS resources unless explicitly provided via host functions.
//! - **Crash Isolation**: A panicking WASM module cannot crash the host process.
//!
//! # Guest ABI
//!
//! WASM plugins must export the following functions:
//!
//! ```text
//! // Required
//! fn plugin_name() -> i32           // Returns ptr to name string
//!
//! // Optional lifecycle hooks
//! fn on_request(ptr: i32, len: i32) -> i32   // Request JSON -> 0=ok, 1=error
//! fn on_response(ptr: i32, len: i32) -> i32  // Response JSON -> 0=ok, 1=error
//! fn on_subgraph_request(ptr: i32, len: i32) -> i32  // Subgraph info JSON -> 0=ok, 1=error
//! ```
//!
//! # Host Functions Available to Guests
//!
//! ```text
//! fn host_log(level: i32, ptr: i32, len: i32)         // Log a message
//! fn host_get_header(ptr: i32, len: i32) -> i64        // Get request header (returns ptr|len)
//! fn host_set_header(key_ptr: i32, key_len: i32,       // Set response header
//!                    val_ptr: i32, val_len: i32)
//! fn host_get_metadata(ptr: i32, len: i32) -> i64      // Get metadata value
//! fn host_set_metadata(key_ptr: i32, key_len: i32,     // Set metadata value
//!                      val_ptr: i32, val_len: i32)
//! ```
//!
//! # Example
//!
//! ```rust,no_run
//! use grpc_graphql_gateway::wasm_plugin::{WasmPluginEngine, WasmPluginConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let engine = WasmPluginEngine::new()?;
//!
//! let config = WasmPluginConfig {
//!     name: "my-auth-plugin".to_string(),
//!     path: "plugins/auth.wasm".into(),
//!     max_memory_bytes: 16 * 1024 * 1024, // 16 MB
//!     max_fuel: 1_000_000,
//!     config: serde_json::json!({"api_key_header": "X-API-Key"}),
//! };
//!
//! let plugin = engine.load_plugin(config)?;
//! // plugin implements the Plugin trait and can be registered with the gateway
//! # Ok(())
//! # }
//! ```

use crate::middleware::Context;
use crate::plugin::Plugin;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use wasmtime::*;

// ────────────────────────────────────────────────────────────────────
// Configuration
// ────────────────────────────────────────────────────────────────────

/// Configuration for a single WASM plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmPluginConfig {
    /// Human-readable name for this plugin instance
    pub name: String,
    /// Path to the `.wasm` file
    pub path: PathBuf,
    /// Maximum memory in bytes the plugin can allocate (default: 16 MB)
    #[serde(default = "default_max_memory")]
    pub max_memory_bytes: usize,
    /// Maximum fuel (instruction budget) per invocation (default: 1M)
    #[serde(default = "default_max_fuel")]
    pub max_fuel: u64,
    /// Arbitrary JSON configuration passed to the plugin on init
    #[serde(default)]
    pub config: serde_json::Value,
}

fn default_max_memory() -> usize {
    16 * 1024 * 1024 // 16 MB
}

fn default_max_fuel() -> u64 {
    1_000_000
}

impl Default for WasmPluginConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            path: PathBuf::new(),
            max_memory_bytes: default_max_memory(),
            max_fuel: default_max_fuel(),
            config: serde_json::Value::Null,
        }
    }
}

/// Resource limits for WASM plugin execution
#[derive(Debug, Clone)]
pub struct WasmResourceLimits {
    /// Maximum memory in bytes
    pub max_memory_bytes: usize,
    /// Maximum fuel per invocation
    pub max_fuel: u64,
    /// Maximum execution time (enforced via fuel, not wall-clock)
    pub max_tables: usize,
    /// Maximum number of table elements
    pub max_table_elements: usize,
    /// Maximum number of WASM instances per plugin
    pub max_instances: u32,
}

impl Default for WasmResourceLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: 16 * 1024 * 1024, // 16 MB
            max_fuel: 1_000_000,
            max_tables: 4,
            max_table_elements: 10_000,
            max_instances: 1,
        }
    }
}

/// Implement wasmtime's ResourceLimiter for our limits
struct PluginResourceLimiter {
    max_memory: usize,
    max_table_elements: usize,
}

impl ResourceLimiter for PluginResourceLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool> {
        if desired > self.max_memory {
            tracing::warn!(
                current_bytes = current,
                desired_bytes = desired,
                max_bytes = self.max_memory,
                "🔒 WASM plugin memory limit exceeded"
            );
            Ok(false)
        } else {
            Ok(true)
        }
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> Result<bool> {
        if desired > self.max_table_elements {
            tracing::warn!(
                current = current,
                desired = desired,
                max = self.max_table_elements,
                "🔒 WASM plugin table limit exceeded"
            );
            Ok(false)
        } else {
            Ok(true)
        }
    }
}

// ────────────────────────────────────────────────────────────────────
// Host State (shared between host functions)
// ────────────────────────────────────────────────────────────────────

/// State shared between the host and guest WASM module.
///
/// This is stored in the wasmtime `Store` and accessible from host functions.
#[derive(Default)]
pub struct HostState {
    /// Request headers (readable by guest)
    pub headers: HashMap<String, String>,
    /// Response metadata to set (writable by guest)
    pub metadata: HashMap<String, String>,
    /// Guest output / error buffer
    pub guest_output: Option<String>,
    /// Plugin configuration (JSON)
    pub plugin_config: serde_json::Value,
    /// Resource limiter
    limiter: Option<PluginResourceLimiter>,
}

impl HostState {
    fn new(config: serde_json::Value, max_memory: usize, max_table_elements: usize) -> Self {
        Self {
            headers: HashMap::new(),
            metadata: HashMap::new(),
            guest_output: None,
            plugin_config: config,
            limiter: Some(PluginResourceLimiter {
                max_memory,
                max_table_elements,
            }),
        }
    }
}

// Implement ResourceLimiterAsync delegation by wrapping in Store
impl ResourceLimiter for HostState {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> Result<bool> {
        if let Some(ref mut limiter) = self.limiter {
            limiter.memory_growing(current, desired, maximum)
        } else {
            Ok(true)
        }
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> Result<bool> {
        if let Some(ref mut limiter) = self.limiter {
            limiter.table_growing(current, desired, maximum)
        } else {
            Ok(true)
        }
    }
}

// ────────────────────────────────────────────────────────────────────
// Engine
// ────────────────────────────────────────────────────────────────────

/// The WASM Plugin Engine manages compilation and instantiation of WASM modules.
///
/// A single engine instance should be shared across the gateway; it handles
/// configuration of the WASM runtime (e.g., fuel metering, memory limits).
#[derive(Clone)]
pub struct WasmPluginEngine {
    engine: Engine,
}

impl WasmPluginEngine {
    /// Create a new WASM plugin engine with default settings.
    pub fn new() -> std::result::Result<Self, crate::error::Error> {
        let mut config = Config::new();
        // Enable fuel-based metering for CPU limits
        config.consume_fuel(true);
        // Async support for non-blocking host calls
        config.async_support(true);
        // Cranelift optimizations for production
        config.cranelift_opt_level(OptLevel::Speed);

        let engine = Engine::new(&config).map_err(|e| {
            crate::error::Error::WasmPlugin(format!("Failed to create WASM engine: {}", e))
        })?;

        tracing::info!("🧩 WASM plugin engine initialized (fuel metering enabled)");

        Ok(Self { engine })
    }

    /// Load and compile a WASM plugin from the given configuration.
    ///
    /// This pre-compiles the module (AOT via Cranelift) so that instantiation
    /// is fast on each request.
    pub fn load_plugin(
        &self,
        config: WasmPluginConfig,
    ) -> std::result::Result<WasmPlugin, crate::error::Error> {
        let wasm_bytes = std::fs::read(&config.path).map_err(|e| {
            crate::error::Error::WasmPlugin(format!(
                "Failed to read WASM file '{}': {}",
                config.path.display(),
                e
            ))
        })?;

        let module = Module::new(&self.engine, &wasm_bytes).map_err(|e| {
            crate::error::Error::WasmPlugin(format!(
                "Failed to compile WASM module '{}': {}",
                config.name, e
            ))
        })?;

        // Validate exported functions
        let has_on_request = module.exports().any(|e| e.name() == "on_request");
        let has_on_response = module.exports().any(|e| e.name() == "on_response");
        let has_on_subgraph = module.exports().any(|e| e.name() == "on_subgraph_request");

        tracing::info!(
            plugin = %config.name,
            path = %config.path.display(),
            max_memory_mb = config.max_memory_bytes / (1024 * 1024),
            max_fuel = config.max_fuel,
            hooks.on_request = has_on_request,
            hooks.on_response = has_on_response,
            hooks.on_subgraph_request = has_on_subgraph,
            "🧩 WASM plugin loaded and compiled"
        );

        Ok(WasmPlugin {
            name: config.name,
            engine: self.engine.clone(),
            module: Arc::new(module),
            max_fuel: config.max_fuel,
            max_memory_bytes: config.max_memory_bytes,
            plugin_config: config.config,
            has_on_request,
            has_on_response,
            has_on_subgraph,
        })
    }

    /// Load a WASM plugin from raw bytes (useful for testing or embedded modules).
    pub fn load_plugin_from_bytes(
        &self,
        name: impl Into<String>,
        wasm_bytes: &[u8],
        limits: WasmResourceLimits,
    ) -> std::result::Result<WasmPlugin, crate::error::Error> {
        let name = name.into();
        let module = Module::new(&self.engine, wasm_bytes).map_err(|e| {
            crate::error::Error::WasmPlugin(format!(
                "Failed to compile WASM module '{}': {}",
                name, e
            ))
        })?;

        let has_on_request = module.exports().any(|e| e.name() == "on_request");
        let has_on_response = module.exports().any(|e| e.name() == "on_response");
        let has_on_subgraph = module.exports().any(|e| e.name() == "on_subgraph_request");

        Ok(WasmPlugin {
            name,
            engine: self.engine.clone(),
            module: Arc::new(module),
            max_fuel: limits.max_fuel,
            max_memory_bytes: limits.max_memory_bytes,
            plugin_config: serde_json::Value::Null,
            has_on_request,
            has_on_response,
            has_on_subgraph,
        })
    }
}

// ────────────────────────────────────────────────────────────────────
// WASM Plugin
// ────────────────────────────────────────────────────────────────────

/// A compiled WASM plugin that implements the gateway's `Plugin` trait.
///
/// Each invocation creates a fresh `Store` with its own memory and fuel budget,
/// ensuring that a misbehaving plugin cannot affect other plugins or the host.
pub struct WasmPlugin {
    name: String,
    engine: Engine,
    module: Arc<Module>,
    max_fuel: u64,
    max_memory_bytes: usize,
    plugin_config: serde_json::Value,
    has_on_request: bool,
    has_on_response: bool,
    has_on_subgraph: bool,
}

impl std::fmt::Debug for WasmPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmPlugin")
            .field("name", &self.name)
            .field("max_fuel", &self.max_fuel)
            .field("max_memory_bytes", &self.max_memory_bytes)
            .field("has_on_request", &self.has_on_request)
            .field("has_on_response", &self.has_on_response)
            .field("has_on_subgraph", &self.has_on_subgraph)
            .finish()
    }
}

impl WasmPlugin {
    /// Create a fresh wasmtime Store with host functions linked.
    fn create_instance(
        &self,
        headers: HashMap<String, String>,
    ) -> std::result::Result<(Store<HostState>, Instance), crate::error::Error> {
        let mut store = Store::new(
            &self.engine,
            HostState::new(
                self.plugin_config.clone(),
                self.max_memory_bytes,
                WasmResourceLimits::default().max_table_elements,
            ),
        );

        // Set resource limits
        store.limiter(|state| state);

        // Add fuel budget
        store
            .set_fuel(self.max_fuel)
            .map_err(|e| crate::error::Error::WasmPlugin(format!("Failed to set fuel: {}", e)))?;

        // Link host functions
        let mut linker = Linker::new(&self.engine);
        Self::link_host_functions(&mut linker)?;

        let instance = linker.instantiate(&mut store, &self.module).map_err(|e| {
            crate::error::Error::WasmPlugin(format!(
                "Failed to instantiate WASM module '{}': {}",
                self.name, e
            ))
        })?;

        // Set headers in state
        store.data_mut().headers = headers;

        Ok((store, instance))
    }

    /// Link all host functions into the linker.
    fn link_host_functions(
        linker: &mut Linker<HostState>,
    ) -> std::result::Result<(), crate::error::Error> {
        // host_log(level: i32, ptr: i32, len: i32)
        linker
            .func_wrap(
                "env",
                "host_log",
                |mut caller: Caller<'_, HostState>, level: i32, ptr: i32, len: i32| {
                    let mem = match caller.get_export("memory") {
                        Some(Extern::Memory(mem)) => mem,
                        _ => return,
                    };
                    let data = mem.data(&caller);
                    let start = ptr as usize;
                    let end = start + len as usize;
                    if end > data.len() {
                        return;
                    }
                    if let Ok(msg) = std::str::from_utf8(&data[start..end]) {
                        match level {
                            0 => tracing::trace!(target: "wasm_plugin", "{}", msg),
                            1 => tracing::debug!(target: "wasm_plugin", "{}", msg),
                            2 => tracing::info!(target: "wasm_plugin", "{}", msg),
                            3 => tracing::warn!(target: "wasm_plugin", "{}", msg),
                            _ => tracing::error!(target: "wasm_plugin", "{}", msg),
                        }
                    }
                },
            )
            .map_err(|e| {
                crate::error::Error::WasmPlugin(format!("Failed to link host_log: {}", e))
            })?;

        // host_get_header(key_ptr: i32, key_len: i32) -> i64
        // Returns (ptr << 32 | len) of the value written to guest memory,
        // or 0 if the header is not found.
        linker
            .func_wrap(
                "env",
                "host_get_header",
                |mut caller: Caller<'_, HostState>, key_ptr: i32, key_len: i32| -> i64 {
                    let mem = match caller.get_export("memory") {
                        Some(Extern::Memory(mem)) => mem,
                        _ => return 0,
                    };

                    // Read the key from guest memory
                    let data = mem.data(&caller);
                    let start = key_ptr as usize;
                    let end = start + key_len as usize;
                    if end > data.len() {
                        return 0;
                    }
                    let key = match std::str::from_utf8(&data[start..end]) {
                        Ok(k) => k.to_string(),
                        Err(_) => return 0,
                    };

                    // Look up the header value
                    let value = match caller.data().headers.get(&key) {
                        Some(v) => v.clone(),
                        None => return 0,
                    };

                    // Write value to guest memory using the alloc export if available
                    let alloc = match caller.get_export("alloc") {
                        Some(Extern::Func(f)) => f,
                        _ => return 0,
                    };

                    let val_bytes = value.as_bytes();
                    let val_len = val_bytes.len() as i32;

                    // Call guest's alloc to get a pointer
                    let mut results = [Val::I32(0)];
                    if alloc
                        .call(&mut caller, &[Val::I32(val_len)], &mut results)
                        .is_err()
                    {
                        return 0;
                    }
                    let val_ptr = results[0].unwrap_i32();

                    // SECURITY: Guard against null pointer returned by guest alloc.
                    if val_ptr == 0 {
                        return 0;
                    }

                    // Write the value bytes
                    let mem = match caller.get_export("memory") {
                        Some(Extern::Memory(mem)) => mem,
                        _ => return 0,
                    };
                    let data = mem.data_mut(&mut caller);
                    let dst_start = val_ptr as usize;
                    let dst_end = dst_start + val_len as usize;
                    if dst_end > data.len() {
                        return 0;
                    }
                    data[dst_start..dst_end].copy_from_slice(val_bytes);

                    // Pack ptr and len into i64
                    ((val_ptr as i64) << 32) | (val_len as i64)
                },
            )
            .map_err(|e| {
                crate::error::Error::WasmPlugin(format!("Failed to link host_get_header: {}", e))
            })?;

        // host_set_header(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32)
        linker
            .func_wrap(
                "env",
                "host_set_header",
                |mut caller: Caller<'_, HostState>,
                 key_ptr: i32,
                 key_len: i32,
                 val_ptr: i32,
                 val_len: i32| {
                    let mem = match caller.get_export("memory") {
                        Some(Extern::Memory(mem)) => mem,
                        _ => return,
                    };
                    let data = mem.data(&caller);

                    let key_start = key_ptr as usize;
                    let key_end = key_start + key_len as usize;
                    let val_start = val_ptr as usize;
                    let val_end = val_start + val_len as usize;

                    if key_end > data.len() || val_end > data.len() {
                        return;
                    }

                    let key = match std::str::from_utf8(&data[key_start..key_end]) {
                        Ok(k) => k.to_string(),
                        Err(_) => return,
                    };
                    let val = match std::str::from_utf8(&data[val_start..val_end]) {
                        Ok(v) => v.to_string(),
                        Err(_) => return,
                    };

                    caller.data_mut().metadata.insert(key, val);
                },
            )
            .map_err(|e| {
                crate::error::Error::WasmPlugin(format!("Failed to link host_set_header: {}", e))
            })?;

        // host_get_metadata(key_ptr: i32, key_len: i32) -> i64
        linker
            .func_wrap(
                "env",
                "host_get_metadata",
                |mut caller: Caller<'_, HostState>, key_ptr: i32, key_len: i32| -> i64 {
                    let mem = match caller.get_export("memory") {
                        Some(Extern::Memory(mem)) => mem,
                        _ => return 0,
                    };

                    let data = mem.data(&caller);
                    let start = key_ptr as usize;
                    let end = start + key_len as usize;
                    if end > data.len() {
                        return 0;
                    }
                    let key = match std::str::from_utf8(&data[start..end]) {
                        Ok(k) => k.to_string(),
                        Err(_) => return 0,
                    };

                    let value = match caller.data().metadata.get(&key) {
                        Some(v) => v.clone(),
                        None => return 0,
                    };

                    let alloc = match caller.get_export("alloc") {
                        Some(Extern::Func(f)) => f,
                        _ => return 0,
                    };

                    let val_bytes = value.as_bytes();
                    let val_len = val_bytes.len() as i32;

                    let mut results = [Val::I32(0)];
                    if alloc
                        .call(&mut caller, &[Val::I32(val_len)], &mut results)
                        .is_err()
                    {
                        return 0;
                    }
                    let val_ptr = results[0].unwrap_i32();

                    // SECURITY: Guard against null pointer returned by guest alloc.
                    if val_ptr == 0 {
                        return 0;
                    }

                    let mem = match caller.get_export("memory") {
                        Some(Extern::Memory(mem)) => mem,
                        _ => return 0,
                    };
                    let data = mem.data_mut(&mut caller);
                    let dst_start = val_ptr as usize;
                    let dst_end = dst_start + val_len as usize;
                    if dst_end > data.len() {
                        return 0;
                    }
                    data[dst_start..dst_end].copy_from_slice(val_bytes);

                    ((val_ptr as i64) << 32) | (val_len as i64)
                },
            )
            .map_err(|e| {
                crate::error::Error::WasmPlugin(format!("Failed to link host_get_metadata: {}", e))
            })?;

        // host_set_metadata(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32)
        linker
            .func_wrap(
                "env",
                "host_set_metadata",
                |mut caller: Caller<'_, HostState>,
                 key_ptr: i32,
                 key_len: i32,
                 val_ptr: i32,
                 val_len: i32| {
                    let mem = match caller.get_export("memory") {
                        Some(Extern::Memory(mem)) => mem,
                        _ => return,
                    };
                    let data = mem.data(&caller);

                    let key_start = key_ptr as usize;
                    let key_end = key_start + key_len as usize;
                    let val_start = val_ptr as usize;
                    let val_end = val_start + val_len as usize;

                    if key_end > data.len() || val_end > data.len() {
                        return;
                    }

                    let key = match std::str::from_utf8(&data[key_start..key_end]) {
                        Ok(k) => k.to_string(),
                        Err(_) => return,
                    };
                    let val = match std::str::from_utf8(&data[val_start..val_end]) {
                        Ok(v) => v.to_string(),
                        Err(_) => return,
                    };

                    caller.data_mut().metadata.insert(key, val);
                },
            )
            .map_err(|e| {
                crate::error::Error::WasmPlugin(format!("Failed to link host_set_metadata: {}", e))
            })?;

        // host_get_config() -> i64
        // Returns a packed (ptr << 32 | len) pointing to the plugin config JSON in guest memory.
        linker
            .func_wrap(
                "env",
                "host_get_config",
                |mut caller: Caller<'_, HostState>| -> i64 {
                    let config_json = serde_json::to_string(&caller.data().plugin_config)
                        .unwrap_or_else(|_| "{}".to_string());

                    let alloc = match caller.get_export("alloc") {
                        Some(Extern::Func(f)) => f,
                        _ => return 0,
                    };

                    let config_bytes = config_json.as_bytes();
                    let config_len = config_bytes.len() as i32;

                    let mut results = [Val::I32(0)];
                    if alloc
                        .call(&mut caller, &[Val::I32(config_len)], &mut results)
                        .is_err()
                    {
                        return 0;
                    }
                    let config_ptr = results[0].unwrap_i32();

                    // SECURITY: Guard against null pointer returned by guest alloc.
                    if config_ptr == 0 {
                        return 0;
                    }

                    let mem = match caller.get_export("memory") {
                        Some(Extern::Memory(mem)) => mem,
                        _ => return 0,
                    };
                    let data = mem.data_mut(&mut caller);
                    let dst_start = config_ptr as usize;
                    let dst_end = dst_start + config_len as usize;
                    if dst_end > data.len() {
                        return 0;
                    }
                    data[dst_start..dst_end].copy_from_slice(config_bytes);

                    ((config_ptr as i64) << 32) | (config_len as i64)
                },
            )
            .map_err(|e| {
                crate::error::Error::WasmPlugin(format!("Failed to link host_get_config: {}", e))
            })?;

        Ok(())
    }

    /// Call a guest function, passing JSON data and returning its result code.
    fn call_guest_fn(
        store: &mut Store<HostState>,
        instance: &Instance,
        fn_name: &str,
        json_data: &[u8],
    ) -> std::result::Result<i32, crate::error::Error> {
        // First, allocate space in guest memory
        let alloc = instance.get_func(&mut *store, "alloc").ok_or_else(|| {
            crate::error::Error::WasmPlugin(
                "WASM module missing 'alloc' export (required for passing data)".to_string(),
            )
        })?;

        let data_len = json_data.len() as i32;
        let mut alloc_results = [Val::I32(0)];
        alloc
            .call(&mut *store, &[Val::I32(data_len)], &mut alloc_results)
            .map_err(|e| crate::error::Error::WasmPlugin(format!("alloc call failed: {}", e)))?;
        let data_ptr = alloc_results[0].unwrap_i32();

        // SECURITY: A return value of 0 signals allocation failure in the guest.
        // Writing to address 0 would corrupt the WASM null page; reject immediately.
        if data_ptr == 0 {
            return Err(crate::error::Error::WasmPlugin(
                "WASM guest alloc returned null pointer (allocation failure)".to_string(),
            ));
        }

        // Write data to guest memory
        let memory = instance.get_memory(&mut *store, "memory").ok_or_else(|| {
            crate::error::Error::WasmPlugin("WASM module missing 'memory' export".to_string())
        })?;
        {
            let mem_data = memory.data_mut(&mut *store);
            let start = data_ptr as usize;
            let end = start + data_len as usize;
            if end > mem_data.len() {
                return Err(crate::error::Error::WasmPlugin(
                    "Not enough guest memory for data".to_string(),
                ));
            }
            mem_data[start..end].copy_from_slice(json_data);
        }

        // Call the guest function
        let func = instance.get_func(&mut *store, fn_name).ok_or_else(|| {
            crate::error::Error::WasmPlugin(format!("Missing WASM export: '{}'", fn_name))
        })?;

        let mut results = [Val::I32(0)];
        func.call(
            &mut *store,
            &[Val::I32(data_ptr), Val::I32(data_len)],
            &mut results,
        )
        .map_err(|e| {
            // Check if this was a fuel exhaustion (OOM / infinite loop protection)
            let msg = e.to_string();
            if msg.contains("fuel") {
                crate::error::Error::WasmPlugin(format!(
                    "WASM plugin exceeded CPU budget (fuel exhausted) in '{}'",
                    fn_name
                ))
            } else {
                crate::error::Error::WasmPlugin(format!("WASM plugin '{}' trapped: {}", fn_name, e))
            }
        })?;

        Ok(results[0].unwrap_i32())
    }
}

// ────────────────────────────────────────────────────────────────────
// Plugin Trait Implementation
// ────────────────────────────────────────────────────────────────────

#[async_trait]
impl Plugin for WasmPlugin {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn name(&self) -> &str {
        &self.name
    }

    async fn on_request(
        &self,
        ctx: &Context,
        req: &async_graphql::Request,
    ) -> Result<(), Self::Error> {
        if !self.has_on_request {
            return Ok(());
        }

        // Serialize context + request into JSON for the guest
        let payload = serde_json::json!({
            "request_id": ctx.request_id,
            "client_ip": ctx.client_ip,
            "query": req.query,
            "operation_name": req.operation_name,
        });
        let json_bytes = serde_json::to_vec(&payload)?;

        // Build headers map from context
        let mut headers = HashMap::new();
        if let Some(ip) = &ctx.client_ip {
            headers.insert("x-client-ip".to_string(), ip.clone());
        }

        let (mut store, instance) = self
            .create_instance(headers)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;

        let result = Self::call_guest_fn(&mut store, &instance, "on_request", &json_bytes)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;

        let fuel_remaining = store.get_fuel().unwrap_or(0);
        let fuel_used = self.max_fuel.saturating_sub(fuel_remaining);

        tracing::debug!(
            plugin = %self.name,
            fuel_used = fuel_used,
            result_code = result,
            "🧩 WASM on_request completed"
        );

        if result != 0 {
            // Guest signaled an error
            let error_msg = store.data().guest_output.clone().unwrap_or_else(|| {
                format!(
                    "WASM plugin '{}' rejected the request (code: {})",
                    self.name, result
                )
            });
            return Err(error_msg.into());
        }

        Ok(())
    }

    async fn on_response(
        &self,
        ctx: &Context,
        res: &async_graphql::Response,
    ) -> Result<(), Self::Error> {
        if !self.has_on_response {
            return Ok(());
        }

        let payload = serde_json::json!({
            "request_id": ctx.request_id,
            "has_errors": !res.errors.is_empty(),
            "error_count": res.errors.len(),
        });
        let json_bytes = serde_json::to_vec(&payload)?;

        let (mut store, instance) = self
            .create_instance(HashMap::new())
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;

        let result = Self::call_guest_fn(&mut store, &instance, "on_response", &json_bytes)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;

        tracing::debug!(
            plugin = %self.name,
            result_code = result,
            "🧩 WASM on_response completed"
        );

        if result != 0 {
            let error_msg = store.data().guest_output.clone().unwrap_or_else(|| {
                format!(
                    "WASM plugin '{}' response hook failed (code: {})",
                    self.name, result
                )
            });
            return Err(error_msg.into());
        }

        Ok(())
    }

    async fn on_schema_build(
        &self,
        _builder: &mut crate::schema::SchemaBuilder,
    ) -> Result<(), Self::Error> {
        // Schema modification from WASM is intentionally limited for safety.
        // WASM plugins can observe schema events but cannot directly mutate
        // the SchemaBuilder (which requires full Rust type access).
        Ok(())
    }

    async fn on_subgraph_request(
        &self,
        service_name: &str,
        metadata: &mut tonic::metadata::MetadataMap,
    ) -> Result<(), Self::Error> {
        if !self.has_on_subgraph {
            return Ok(());
        }

        // Build current metadata as headers map
        let mut current_headers = HashMap::new();
        for key_and_value in metadata.iter() {
            if let tonic::metadata::KeyAndValueRef::Ascii(key, value) = key_and_value {
                if let Ok(v) = value.to_str() {
                    current_headers.insert(key.as_str().to_string(), v.to_string());
                }
            }
        }

        let payload = serde_json::json!({
            "service_name": service_name,
            "headers": current_headers,
        });
        let json_bytes = serde_json::to_vec(&payload)?;

        let (mut store, instance) = self
            .create_instance(current_headers)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;

        let result = Self::call_guest_fn(&mut store, &instance, "on_subgraph_request", &json_bytes)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;

        // Apply metadata set by the guest back to the MetadataMap
        for (key, value) in &store.data().metadata {
            if let (Ok(k), Ok(v)) = (
                key.parse::<tonic::metadata::MetadataKey<tonic::metadata::Ascii>>(),
                value.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>(),
            ) {
                metadata.insert(k, v);
            }
        }

        tracing::debug!(
            plugin = %self.name,
            service = %service_name,
            metadata_changes = store.data().metadata.len(),
            result_code = result,
            "🧩 WASM on_subgraph_request completed"
        );

        if result != 0 {
            let error_msg = store.data().guest_output.clone().unwrap_or_else(|| {
                format!(
                    "WASM plugin '{}' rejected subgraph request to '{}' (code: {})",
                    self.name, service_name, result
                )
            });
            return Err(error_msg.into());
        }

        Ok(())
    }
}

// Make WasmPlugin Send + Sync safe
// wasmtime::Module is Send + Sync, Engine is Send + Sync
unsafe impl Send for WasmPlugin {}
unsafe impl Sync for WasmPlugin {}

// ────────────────────────────────────────────────────────────────────
// WASM Plugin Manager
// ────────────────────────────────────────────────────────────────────

/// Manages the lifecycle of multiple WASM plugins.
///
/// Handles loading, hot-reloading (planned), and health-checking of WASM plugins.
#[derive(Clone)]
pub struct WasmPluginManager {
    engine: WasmPluginEngine,
    plugins: Arc<Mutex<Vec<Arc<WasmPlugin>>>>,
}

impl WasmPluginManager {
    /// Create a new WASM plugin manager.
    pub fn new() -> std::result::Result<Self, crate::error::Error> {
        Ok(Self {
            engine: WasmPluginEngine::new()?,
            plugins: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Load a plugin from configuration and add it to the registry.
    pub fn load_and_register(
        &self,
        config: WasmPluginConfig,
    ) -> std::result::Result<Arc<WasmPlugin>, crate::error::Error> {
        let plugin = Arc::new(self.engine.load_plugin(config)?);
        let mut plugins = self.plugins.lock().map_err(|e| {
            crate::error::Error::WasmPlugin(format!("Plugin manager lock poisoned: {}", e))
        })?;
        plugins.push(plugin.clone());
        Ok(plugin)
    }

    /// Get the underlying engine for custom loading.
    pub fn engine(&self) -> &WasmPluginEngine {
        &self.engine
    }

    /// List all loaded plugin names.
    pub fn plugin_names(&self) -> Vec<String> {
        self.plugins
            .lock()
            .map(|plugins| plugins.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default()
    }

    /// Get the number of loaded plugins.
    pub fn plugin_count(&self) -> usize {
        self.plugins
            .lock()
            .map(|plugins| plugins.len())
            .unwrap_or(0)
    }
}

// ────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_plugin_config_defaults() {
        let config = WasmPluginConfig::default();
        assert_eq!(config.max_memory_bytes, 16 * 1024 * 1024);
        assert_eq!(config.max_fuel, 1_000_000);
        assert!(config.name.is_empty());
    }

    #[test]
    fn test_wasm_resource_limits_defaults() {
        let limits = WasmResourceLimits::default();
        assert_eq!(limits.max_memory_bytes, 16 * 1024 * 1024);
        assert_eq!(limits.max_fuel, 1_000_000);
        assert_eq!(limits.max_tables, 4);
        assert_eq!(limits.max_table_elements, 10_000);
        assert_eq!(limits.max_instances, 1);
    }

    #[test]
    fn test_wasm_engine_creation() {
        let engine = WasmPluginEngine::new();
        assert!(engine.is_ok(), "WASM engine should be created successfully");
    }

    #[test]
    fn test_plugin_config_serialization() {
        let config = WasmPluginConfig {
            name: "test-plugin".to_string(),
            path: PathBuf::from("/tmp/test.wasm"),
            max_memory_bytes: 8 * 1024 * 1024,
            max_fuel: 500_000,
            config: serde_json::json!({"key": "value"}),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: WasmPluginConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test-plugin");
        assert_eq!(deserialized.max_fuel, 500_000);
    }

    #[test]
    fn test_host_state_default() {
        let state = HostState::default();
        assert!(state.headers.is_empty());
        assert!(state.metadata.is_empty());
        assert!(state.guest_output.is_none());
    }

    #[test]
    fn test_plugin_manager_creation() {
        let manager = WasmPluginManager::new();
        assert!(manager.is_ok());
        let manager = manager.unwrap();
        assert_eq!(manager.plugin_count(), 0);
        assert!(manager.plugin_names().is_empty());
    }

    #[test]
    fn test_load_invalid_wasm_file() {
        let engine = WasmPluginEngine::new().unwrap();
        let config = WasmPluginConfig {
            name: "bad-plugin".to_string(),
            path: PathBuf::from("/nonexistent/path.wasm"),
            ..Default::default()
        };
        let result = engine.load_plugin(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Failed to read WASM file"));
    }

    #[test]
    fn test_resource_limiter_memory() {
        let mut limiter = PluginResourceLimiter {
            max_memory: 1024,
            max_table_elements: 100,
        };

        // Should allow growth within limits
        assert!(limiter.memory_growing(0, 512, None).unwrap());
        assert!(limiter.memory_growing(512, 1024, None).unwrap());

        // Should deny growth beyond limits
        assert!(!limiter.memory_growing(1024, 2048, None).unwrap());
    }

    #[test]
    fn test_resource_limiter_table() {
        let mut limiter = PluginResourceLimiter {
            max_memory: 1024,
            max_table_elements: 100,
        };

        assert!(limiter.table_growing(0, 50, None).unwrap());
        assert!(limiter.table_growing(50, 100, None).unwrap());
        assert!(!limiter.table_growing(100, 200, None).unwrap());
    }
}
