//! Graceful shutdown support for the GraphQL gateway.
//!
//! This module provides utilities for gracefully shutting down the gateway,
//! ensuring that in-flight requests complete and resources are properly cleaned up.
//!
//! # Features
//!
//! - Signal handling (SIGTERM, SIGINT, Ctrl+C)
//! - Configurable shutdown timeout
//! - In-flight request draining
//! - Subscription cleanup
//!
//! # Example
//!
//! ```rust,no_run
//! use grpc_graphql_gateway::{Gateway, ShutdownConfig};
//! use std::time::Duration;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! Gateway::builder()
//!     // ... configuration ...
//!     .with_graceful_shutdown(ShutdownConfig {
//!         timeout: Duration::from_secs(30),
//!         ..Default::default()
//!     })
//!     .serve("0.0.0.0:8080")
//!     .await?;
//! # Ok(())
//! # }
//! ```

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, warn};

/// Configuration for graceful shutdown behavior.
#[derive(Debug, Clone)]
pub struct ShutdownConfig {
    /// Maximum time to wait for in-flight requests to complete (default: 30 seconds)
    pub timeout: Duration,
    /// Whether to handle OS signals (SIGTERM, SIGINT) automatically (default: true)
    pub handle_signals: bool,
    /// Grace period before forceful shutdown after timeout (default: 5 seconds)
    pub force_shutdown_delay: Duration,
}

impl Default for ShutdownConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            handle_signals: true,
            force_shutdown_delay: Duration::from_secs(5),
        }
    }
}

/// Shutdown coordinator that manages the graceful shutdown process.
#[derive(Clone)]
pub struct ShutdownCoordinator {
    /// Signal to stop accepting new connections
    shutdown_tx: broadcast::Sender<()>,
    /// Watch channel for shutdown state
    state_tx: Arc<watch::Sender<ShutdownState>>,
    state_rx: watch::Receiver<ShutdownState>,
    /// Count of active connections/requests
    active_count: Arc<AtomicUsize>,
    /// Whether shutdown has been initiated
    is_shutting_down: Arc<AtomicBool>,
    /// Configuration
    config: ShutdownConfig,
}

/// Current state of the shutdown process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownState {
    /// Server is running normally
    Running,
    /// Shutdown initiated, draining requests
    Draining,
    /// Shutdown complete
    Shutdown,
}

impl ShutdownCoordinator {
    /// Create a new shutdown coordinator with the given configuration.
    pub fn new(config: ShutdownConfig) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        let (state_tx, state_rx) = watch::channel(ShutdownState::Running);

        Self {
            shutdown_tx,
            state_tx: Arc::new(state_tx),
            state_rx,
            active_count: Arc::new(AtomicUsize::new(0)),
            is_shutting_down: Arc::new(AtomicBool::new(false)),
            config,
        }
    }

    /// Create a shutdown coordinator with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(ShutdownConfig::default())
    }

    /// Check if shutdown has been initiated.
    pub fn is_shutting_down(&self) -> bool {
        self.is_shutting_down.load(Ordering::SeqCst)
    }

    /// Get the current number of active requests/connections.
    pub fn active_count(&self) -> usize {
        self.active_count.load(Ordering::SeqCst)
    }

    /// Subscribe to shutdown signals.
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.shutdown_tx.subscribe()
    }

    /// Watch the shutdown state.
    pub fn watch_state(&self) -> watch::Receiver<ShutdownState> {
        self.state_rx.clone()
    }

    /// Increment the active request count (call when a request starts).
    pub fn request_started(&self) {
        self.active_count.fetch_add(1, Ordering::SeqCst);
    }

    /// Decrement the active request count (call when a request completes).
    pub fn request_completed(&self) {
        let prev = self.active_count.fetch_sub(1, Ordering::SeqCst);
        debug!(active_count = prev - 1, "Request completed");
    }

    /// Create a guard that automatically tracks request lifecycle.
    pub fn request_guard(&self) -> RequestGuard {
        self.request_started();
        RequestGuard {
            coordinator: self.clone(),
        }
    }

    /// Initiate graceful shutdown.
    ///
    /// This will:
    /// 1. Stop accepting new connections
    /// 2. Wait for in-flight requests to complete (up to timeout)
    /// 3. Force shutdown if timeout is exceeded
    pub async fn shutdown(&self) {
        if self
            .is_shutting_down
            .swap(true, Ordering::SeqCst)
        {
            debug!("Shutdown already in progress");
            return;
        }

        info!("Initiating graceful shutdown...");

        // Update state to draining
        let _ = self.state_tx.send(ShutdownState::Draining);

        // Notify all subscribers
        let _ = self.shutdown_tx.send(());

        // Wait for active requests to complete
        let start = std::time::Instant::now();
        let timeout = self.config.timeout;

        loop {
            let active = self.active_count();

            if active == 0 {
                info!("All requests completed, shutting down");
                break;
            }

            if start.elapsed() >= timeout {
                warn!(
                    active_count = active,
                    "Shutdown timeout reached, {} requests still active",
                    active
                );
                break;
            }

            debug!(
                active_count = active,
                elapsed_secs = start.elapsed().as_secs(),
                "Waiting for {} active request(s) to complete",
                active
            );

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Force shutdown delay
        if self.active_count() > 0 {
            warn!(
                "Waiting {} seconds before forcing shutdown...",
                self.config.force_shutdown_delay.as_secs()
            );
            tokio::time::sleep(self.config.force_shutdown_delay).await;
        }

        // Update state to shutdown
        let _ = self.state_tx.send(ShutdownState::Shutdown);

        info!("Graceful shutdown complete");
    }

    /// Create a future that completes when shutdown is signaled.
    ///
    /// This is useful for passing to `axum::serve().with_graceful_shutdown()`.
    pub fn shutdown_signal(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut rx = self.subscribe();
        async move {
            let _ = rx.recv().await;
        }
    }
}

/// RAII guard that tracks request lifecycle.
///
/// Automatically decrements the active count when dropped.
pub struct RequestGuard {
    coordinator: ShutdownCoordinator,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.coordinator.request_completed();
    }
}

/// Create a future that completes on SIGTERM or SIGINT (Ctrl+C).
///
/// This is useful for Unix-like platforms where you want to handle
/// OS shutdown signals.
#[cfg(unix)]
pub async fn signal_shutdown() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");

    tokio::select! {
        _ = sigterm.recv() => {
            info!("Received SIGTERM");
        }
        _ = sigint.recv() => {
            info!("Received SIGINT (Ctrl+C)");
        }
    }
}

/// Create a future that completes on Ctrl+C.
///
/// This works on all platforms.
#[cfg(not(unix))]
pub async fn signal_shutdown() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install Ctrl+C handler");
    info!("Received Ctrl+C");
}

/// Create a combined shutdown signal that triggers on OS signals.
pub fn os_signal_shutdown() -> impl Future<Output = ()> + Send + 'static {
    async {
        signal_shutdown().await;
    }
}

/// Helper to run a server with graceful shutdown.
///
/// # Example
///
/// ```rust,no_run
/// use grpc_graphql_gateway::shutdown::{run_with_graceful_shutdown, ShutdownConfig};
/// use axum::Router;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let app = Router::new();
/// let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
///
/// run_with_graceful_shutdown(
///     listener,
///     app,
///     ShutdownConfig::default(),
/// ).await?;
/// # Ok(())
/// # }
/// ```
pub async fn run_with_graceful_shutdown(
    listener: tokio::net::TcpListener,
    app: axum::Router,
    config: ShutdownConfig,
) -> std::io::Result<()> {
    let coordinator = ShutdownCoordinator::new(config.clone());
    let coordinator_clone = coordinator.clone();

    // Spawn a task to handle OS signals
    if config.handle_signals {
        let coordinator_for_signal = coordinator.clone();
        tokio::spawn(async move {
            signal_shutdown().await;
            coordinator_for_signal.shutdown().await;
        });
    }

    // Run the server with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(coordinator_clone.shutdown_signal())
        .await?;

    // Ensure shutdown completes
    coordinator.shutdown().await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shutdown_coordinator_creation() {
        let coordinator = ShutdownCoordinator::with_defaults();
        assert!(!coordinator.is_shutting_down());
        assert_eq!(coordinator.active_count(), 0);
    }

    #[tokio::test]
    async fn test_request_tracking() {
        let coordinator = ShutdownCoordinator::with_defaults();

        coordinator.request_started();
        assert_eq!(coordinator.active_count(), 1);

        coordinator.request_started();
        assert_eq!(coordinator.active_count(), 2);

        coordinator.request_completed();
        assert_eq!(coordinator.active_count(), 1);

        coordinator.request_completed();
        assert_eq!(coordinator.active_count(), 0);
    }

    #[tokio::test]
    async fn test_request_guard() {
        let coordinator = ShutdownCoordinator::with_defaults();
        assert_eq!(coordinator.active_count(), 0);

        {
            let _guard = coordinator.request_guard();
            assert_eq!(coordinator.active_count(), 1);
        }

        // Guard dropped, count should be back to 0
        assert_eq!(coordinator.active_count(), 0);
    }

    #[tokio::test]
    async fn test_shutdown_with_no_active_requests() {
        let config = ShutdownConfig {
            timeout: Duration::from_millis(100),
            handle_signals: false,
            force_shutdown_delay: Duration::from_millis(10),
        };
        let coordinator = ShutdownCoordinator::new(config);

        // Shutdown should complete immediately with no active requests
        let start = std::time::Instant::now();
        coordinator.shutdown().await;
        assert!(start.elapsed() < Duration::from_millis(50));
        assert!(coordinator.is_shutting_down());
    }

    #[tokio::test]
    async fn test_shutdown_waits_for_active_requests() {
        let config = ShutdownConfig {
            timeout: Duration::from_secs(5),
            handle_signals: false,
            force_shutdown_delay: Duration::from_millis(10),
        };
        let coordinator = ShutdownCoordinator::new(config);

        // Simulate an active request
        let coordinator_clone = coordinator.clone();
        tokio::spawn(async move {
            let _guard = coordinator_clone.request_guard();
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        // Give time for the guard to be created
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(coordinator.active_count(), 1);

        // Shutdown should wait for the request to complete
        coordinator.shutdown().await;
        assert_eq!(coordinator.active_count(), 0);
    }

    #[tokio::test]
    async fn test_shutdown_state_transitions() {
        let config = ShutdownConfig {
            timeout: Duration::from_millis(100),
            handle_signals: false,
            force_shutdown_delay: Duration::from_millis(10),
        };
        let coordinator = ShutdownCoordinator::new(config);

        let mut state_rx = coordinator.watch_state();
        assert_eq!(*state_rx.borrow(), ShutdownState::Running);

        coordinator.shutdown().await;

        // After shutdown, state should be Shutdown
        assert_eq!(*state_rx.borrow(), ShutdownState::Shutdown);
    }
}
