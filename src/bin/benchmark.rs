use std::env;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Barrier;
use uuid::Uuid;

const SERVER_URL: &str = "http://127.0.0.1:8888/graphql";
const CONCURRENCY: usize = 50;
const DURATION_SECS: u64 = 10;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let uncached = args.contains(&"--disable-cache".to_string());

    println!("Starting benchmark against {}", SERVER_URL);
    println!("Mode: {}", if uncached { "Uncached (Full Integration)" } else { "Cached (Gateway Overhead)" });
    println!("Concurrency: {}", CONCURRENCY);
    println!("Duration: {} seconds", DURATION_SECS);

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(CONCURRENCY)
        .build()?;

    let start_time = Instant::now();
    let end_time = start_time + Duration::from_secs(DURATION_SECS);
    
    let total_requests = Arc::new(AtomicUsize::new(0));
    let total_errors = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(CONCURRENCY));
    
    let mut handles = Vec::new();

    for _ in 0..CONCURRENCY {
        let client = client.clone();
        let total_requests = total_requests.clone();
        let total_errors = total_errors.clone();
        let barrier = barrier.clone();

        handles.push(tokio::spawn(async move {
            barrier.wait().await; // Wait for all tasks to be ready
            
            while Instant::now() < end_time {
                let query = if uncached {
                    let random_name = Uuid::new_v4().to_string();
                    serde_json::json!({
                        "query": "query($n: String!) { hello(name: $n) { message } }",
                        "variables": { "n": random_name }
                    })
                } else {
                    serde_json::json!({
                        "query": "{ hello(name: \"Benchmark\") { message } }"
                    })
                };

                match client.post(SERVER_URL).json(&query).send().await {
                    Ok(resp) => {
                        if resp.status().is_success() {
                            total_requests.fetch_add(1, Ordering::Relaxed);
                        } else {
                            total_errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(_) => {
                        total_errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    // Wait for all tasks to complete
    for handle in handles {
        let _ = handle.await;
    }

    let duration = start_time.elapsed();
    let requests = total_requests.load(Ordering::Relaxed);
    let errors = total_errors.load(Ordering::Relaxed);
    
    let rps = requests as f64 / duration.as_secs_f64();

    println!("\nBenchmark Complete!");
    println!("Time taken: {:.2?}", duration);
    println!("Total Requests: {}", requests);
    println!("Total Errors: {}", errors);
    println!("Requests/sec: {:.2}", rps);
    
    if errors > 0 {
        println!("WARNING: There were {} errors.", errors);
    }

    Ok(())
}
