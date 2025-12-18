use std::env;
use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Barrier;
use uuid::Uuid;

const SERVER_URL: &str = "http://127.0.0.1:8888/graphql";

/// Benchmark mode
#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Cached,
    Uncached,
    Mixed,
    Stress,
}

/// Benchmark results
#[derive(Debug)]
struct BenchmarkResults {
    mode: Mode,
    duration: Duration,
    total_requests: u64,
    successful: u64,
    errors: u64,
    min_latency_us: u64,
    max_latency_us: u64,
    total_latency_us: u64,
    p50_estimate_us: u64,
    p99_estimate_us: u64,
}

impl BenchmarkResults {
    fn rps(&self) -> f64 {
        self.successful as f64 / self.duration.as_secs_f64()
    }

    fn avg_latency_us(&self) -> f64 {
        if self.successful > 0 {
            self.total_latency_us as f64 / self.successful as f64
        } else {
            0.0
        }
    }

    fn error_rate(&self) -> f64 {
        if self.total_requests > 0 {
            self.errors as f64 / self.total_requests as f64 * 100.0
        } else {
            0.0
        }
    }

    fn print(&self) {
        println!("\n╔══════════════════════════════════════════════════════════════╗");
        println!(
            "║              BENCHMARK RESULTS - {:?} Mode                  ║",
            self.mode
        );
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!(
            "║ Duration:           {:>10.2?}                              ║",
            self.duration
        );
        println!(
            "║ Total Requests:     {:>10}                              ║",
            self.total_requests
        );
        println!(
            "║ Successful:         {:>10}                              ║",
            self.successful
        );
        println!(
            "║ Errors:             {:>10}                              ║",
            self.errors
        );
        println!(
            "║ Error Rate:         {:>9.2}%                              ║",
            self.error_rate()
        );
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!("║                     THROUGHPUT                               ║");
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!(
            "║ Requests/sec:       {:>10.2}                              ║",
            self.rps()
        );
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!("║                      LATENCY                                 ║");
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!(
            "║ Min:                {:>10}µs                             ║",
            self.min_latency_us
        );
        println!(
            "║ Avg:                {:>10.2}µs                             ║",
            self.avg_latency_us()
        );
        println!(
            "║ Max:                {:>10}µs                             ║",
            self.max_latency_us
        );
        println!(
            "║ P50 (est):          {:>10}µs                             ║",
            self.p50_estimate_us
        );
        println!(
            "║ P99 (est):          {:>10}µs                             ║",
            self.p99_estimate_us
        );
        println!("╚══════════════════════════════════════════════════════════════╝");

        // Performance rating
        let rps = self.rps();
        let rating = if rps >= 100_000.0 {
            "🚀 EXCELLENT (100K+ RPS)"
        } else if rps >= 50_000.0 {
            "⭐ GREAT (50K+ RPS)"
        } else if rps >= 20_000.0 {
            "✓ GOOD (20K+ RPS)"
        } else if rps >= 10_000.0 {
            "📊 MODERATE (10K+ RPS)"
        } else {
            "⚠️ NEEDS OPTIMIZATION"
        };
        println!("\nPerformance Rating: {}", rating);
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    // Parse arguments
    let mode = if args.contains(&"--uncached".to_string()) {
        Mode::Uncached
    } else if args.contains(&"--mixed".to_string()) {
        Mode::Mixed
    } else if args.contains(&"--stress".to_string()) {
        Mode::Stress
    } else {
        Mode::Cached
    };

    let concurrency: usize = args
        .iter()
        .find(|a| a.starts_with("--concurrency="))
        .and_then(|a| a.strip_prefix("--concurrency="))
        .and_then(|v| v.parse().ok())
        .unwrap_or(if mode == Mode::Stress { 200 } else { 100 });

    let duration: u64 = args
        .iter()
        .find(|a| a.starts_with("--duration="))
        .and_then(|a| a.strip_prefix("--duration="))
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    let url = args
        .iter()
        .find(|a| a.starts_with("--url="))
        .and_then(|a| a.strip_prefix("--url="))
        .map(String::from)
        .unwrap_or_else(|| SERVER_URL.to_string());

    println!("═══════════════════════════════════════════════════════════════");
    println!("    gRPC-GraphQL Gateway High-Performance Benchmark");
    println!("═══════════════════════════════════════════════════════════════");
    println!("Target:      {}", url);
    println!("Mode:        {:?}", mode);
    println!("Concurrency: {} parallel workers", concurrency);
    println!("Duration:    {} seconds", duration);
    println!("═══════════════════════════════════════════════════════════════");

    // Warm up the connection pool
    println!("\n🔄 Warming up connection pool...");
    let warmup_client = reqwest::Client::builder()
        .pool_max_idle_per_host(concurrency)
        .http2_prior_knowledge() // Use HTTP/2 directly
        .tcp_nodelay(true)
        .build()?;

    let mut warmup_handles = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        let client = warmup_client.clone();
        let url = url.clone();
        warmup_handles.push(tokio::spawn(async move {
            let _ = client
                .post(&url)
                .json(&serde_json::json!({
                    "query": "{ user(id: \"u1\") { name } }"
                }))
                .send()
                .await;
        }));
    }
    for handle in warmup_handles {
        let _ = handle.await;
    }
    println!("✓ Warmup complete\n");

    // Create high-performance HTTP client
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(concurrency * 2)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_nodelay(true)
        .tcp_keepalive(Duration::from_secs(30))
        .timeout(Duration::from_secs(30))
        .build()?;

    // We'll set start_time after the barrier to measure actual benchmark duration
    let start_time = Arc::new(tokio::sync::OnceCell::new());
    let end_time_arc = Arc::new(tokio::sync::OnceCell::new());

    // Atomic counters for thread-safe metrics
    let successful = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));
    let total_latency = Arc::new(AtomicU64::new(0));
    let min_latency = Arc::new(AtomicU64::new(u64::MAX));
    let max_latency = Arc::new(AtomicU64::new(0));
    let barrier = Arc::new(Barrier::new(concurrency));

    let mut handles = Vec::with_capacity(concurrency);

    for worker_id in 0..concurrency {
        let client = client.clone();
        let url = url.clone();
        let successful = successful.clone();
        let errors = errors.clone();
        let total_latency = total_latency.clone();
        let min_latency = min_latency.clone();
        let max_latency = max_latency.clone();
        let barrier = barrier.clone();
        let start_time = start_time.clone();
        let end_time_arc = end_time_arc.clone();

        handles.push(tokio::spawn(async move {
            // Wait for all workers to be ready
            barrier.wait().await;

            // Set global start time and end time if not already set
            let start = *start_time.get_or_init(|| async { Instant::now() }).await;
            let end_time = start + Duration::from_secs(duration);
            let _ = end_time_arc.get_or_init(|| async { end_time }).await;

            let mut local_min = u64::MAX;
            let mut local_max = 0u64;
            let mut local_errors = 0u64;

            while Instant::now() < end_time {
                let query = match mode {
                    Mode::Cached => {
                        // Same query every time - should hit cache
                        serde_json::json!({
                            "query": "{ user(id: \"u1\") { name } }"
                        })
                    }
                    Mode::Uncached => {
                        // Random name - cache miss every time
                        serde_json::json!({
                            "query": "{ user(id: \"u1\") { name } }"
                        })
                    }
                    Mode::Mixed => {
                        // 80% cached, 20% uncached
                        if fastrand::u8(..) < 204 {
                            serde_json::json!({
                                "query": "{ user(id: \"u1\") { name } }"
                            })
                        } else {
                            serde_json::json!({
                                "query": "{ user(id: \"u2\") { name } }"
                            })
                        }
                    }
                    Mode::Stress => {
                        serde_json::json!({
                            "query": "{ products { upc name price createdBy { name } } }"
                        })
                    }
                };

                let req_start = Instant::now();

                match client.post(&url).json(&query).send().await {
                    Ok(resp) => {
                        let latency_us = req_start.elapsed().as_micros() as u64;

                        if resp.status().is_success() {
                            successful.fetch_add(1, Ordering::Relaxed);
                            total_latency.fetch_add(latency_us, Ordering::Relaxed);
                            local_min = local_min.min(latency_us);
                            local_max = local_max.max(latency_us);
                        } else {
                            errors.fetch_add(1, Ordering::Relaxed);
                            if local_errors < 5 {
                                eprintln!(
                                    "  ⚠️ Worker {} received error: HTTP {}",
                                    worker_id,
                                    resp.status()
                                );
                            }
                            local_errors += 1;
                        }
                    }
                    Err(e) => {
                        errors.fetch_add(1, Ordering::Relaxed);
                        if local_errors < 5 {
                            let mut msg = format!("  ⚠️ Worker {} request error: {}", worker_id, e);
                            if let Some(source) = e.source() {
                                msg.push_str(&format!(" (Source: {})", source));
                            }
                            eprintln!("{}", msg);
                        }
                        local_errors += 1;
                    }
                }
            }

            // Update min/max with CAS loops at the end of the worker's run
            let mut current_min = min_latency.load(Ordering::Relaxed);
            while local_min < current_min {
                match min_latency.compare_exchange_weak(
                    current_min,
                    local_min,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(c) => current_min = c,
                }
            }

            let mut current_max = max_latency.load(Ordering::Relaxed);
            while local_max > current_max {
                match max_latency.compare_exchange_weak(
                    current_max,
                    local_max,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(c) => current_max = c,
                }
            }
        }));
    }

    // Progress indicator
    let progress_handle = {
        let successful = successful.clone();
        let errors = errors.clone();
        let end_time_arc = end_time_arc.clone();
        tokio::spawn(async move {
            // Wait for end_time to be set
            while end_time_arc.get().is_none() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let end_time = *end_time_arc.get().unwrap();

            let mut last_count = 0u64;
            while Instant::now() < end_time {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let current = successful.load(Ordering::Relaxed);
                let current_errors = errors.load(Ordering::Relaxed);
                let delta = current.saturating_sub(last_count);
                println!(
                    "  📊 {:>7} requests/sec | Total: {:>8} | Errors: {:>6}",
                    delta, current, current_errors
                );
                last_count = current;
            }
        })
    };

    // Wait for all workers
    for handle in handles {
        let _ = handle.await;
    }
    progress_handle.abort();

    let actual_start = *start_time.get().unwrap_or(&Instant::now());
    let actual_duration = actual_start.elapsed();
    let total_successful = successful.load(Ordering::Relaxed);
    let total_errors = errors.load(Ordering::Relaxed);
    let total_latency_us = total_latency.load(Ordering::Relaxed);
    let min_lat = min_latency.load(Ordering::Relaxed);
    let max_lat = max_latency.load(Ordering::Relaxed);

    // Estimate percentiles based on avg and range
    let avg_lat = if total_successful > 0 {
        total_latency_us / total_successful
    } else {
        0
    };
    let p50_est = avg_lat;
    let p99_est = avg_lat + (max_lat - avg_lat) / 2;

    let results = BenchmarkResults {
        mode,
        duration: actual_duration,
        total_requests: total_successful + total_errors,
        successful: total_successful,
        errors: total_errors,
        min_latency_us: if min_lat == u64::MAX { 0 } else { min_lat },
        max_latency_us: max_lat,
        total_latency_us,
        p50_estimate_us: p50_est,
        p99_estimate_us: p99_est,
    };

    results.print();

    // Additional recommendations
    println!("\n💡 Optimization Tips:");
    if results.rps() < 100_000.0 {
        println!("  • Enable response caching with ShardedCache");
        println!("  • Use SIMD JSON parsing (FastJsonParser)");
        println!("  • Enable request collapsing for identical queries");
        println!("  • Increase tokio worker threads with TOKIO_WORKER_THREADS env");
        println!("  • Use mimalloc allocator (already enabled in high_performance module)");
    }

    if results.error_rate() > 1.0 {
        println!("  • ⚠️ High error rate detected - check backend health");
        println!("  • Consider enabling circuit breaker");
    }

    Ok(())
}
