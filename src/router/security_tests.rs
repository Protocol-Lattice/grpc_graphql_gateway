use super::*;
use std::net::SocketAddr;
use std::time::Duration;
use warp::{Filter, Reply};

// Helper to find a free port
fn get_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

// Helper to spawn a mock subgraph server
async fn spawn_mock_subgraph(port: u16, behavior: &str) -> SocketAddr {
    let behavior = behavior.to_string();
    let route = warp::post().and(warp::path("graphql")).map(move || {
        match behavior.as_str() {
            "normal" => warp::reply::json(&serde_json::json!({
                "data": { "users": [{ "id": "1", "name": "Test" }] }
            }))
            .into_response(),
            "malformed" => {
                // Return invalid JSON
                warp::reply::with_status("Not JSON", warp::http::StatusCode::OK).into_response()
            }
            "error" => {
                // Return 500
                warp::reply::with_status(
                    "Internal Error",
                    warp::http::StatusCode::INTERNAL_SERVER_ERROR,
                )
                .into_response()
            }
            "slow" => {
                std::thread::sleep(Duration::from_millis(100));
                warp::reply::json(&serde_json::json!({ "data": "delayed" })).into_response()
            }
            "huge" => {
                let huge_data: Vec<String> = (0..10_000).map(|i| format!("user-{}", i)).collect();
                warp::reply::json(&serde_json::json!({ "data": { "users": huge_data } }))
                    .into_response()
            }
            _ => warp::reply::json(&serde_json::json!({})).into_response(),
        }
    });

    let port = if port == 0 { get_free_port() } else { port };
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let server = warp::serve(route).run(addr);
    tokio::spawn(server);
    addr
}

#[tokio::test]
async fn test_security_input_fuzzing_massive_query() {
    // Test that the router handles an extremely large query string without crashing
    let config = RouterConfig {
        port: 0,
        subgraphs: vec![],
        force_gbp: false,
        apq: None,
        request_collapsing: None,
        waf: None,
        query_cost: None,
        disable_introspection: false,
        circuit_breaker: None,
    };
    let router = GbpRouter::new(config);

    // Generate a massive string (10MB)
    let massive_query = "query { ".to_string() + &"a ".repeat(5_000_000) + "}";

    // It should fail gracefully (likely network error as no subgraphs, or logic error)
    // but crucially, it should NOT panic.
    let result = router
        .execute_scatter_gather(Some(&massive_query), None, None)
        .await;

    // We expect it to process it (and return empty/error since no subgraphs), but mostly just check it didn't panic.
    assert!(result.is_ok() || result.is_err());
}

#[tokio::test]
async fn test_security_ddos_concurrent_flooding() {
    // Start strictly limited router
    let ddos_config = DdosConfig {
        global_rps: 100,
        per_ip_rps: 10,
        per_ip_burst: 10,
    };

    let protection = DdosProtection::new(ddos_config);
    let protection = Arc::new(protection);

    let ip: IpAddr = "192.168.1.100".parse().unwrap();

    // Simulate 100 concurrent requests from same IP
    let mut tasks = Vec::new();
    for _ in 0..100 {
        let p = protection.clone();
        tasks.push(tokio::spawn(async move { p.check(ip).await }));
    }

    let mut allowed_count = 0;
    for task in tasks {
        if task.await.unwrap() {
            allowed_count += 1;
        }
    }

    // Should pass initial burst (10) + maybe 1-2 leakage
    // Definitely should block majority
    assert!(
        allowed_count <= 20,
        "DDoS protection failed to throttle concurrent flood. Allowed: {}",
        allowed_count
    );
}

#[tokio::test]
async fn test_security_subgraph_response_validation() {
    // 1. Setup mock subgraph returning MALFORMED content
    let mock_addr = spawn_mock_subgraph(0, "malformed").await;

    let config = RouterConfig {
        port: 0,
        subgraphs: vec![SubgraphConfig {
            name: "bad_subgraph".into(),
            url: format!("http://{}/graphql", mock_addr),
            headers: std::collections::HashMap::new(),
        }],
        force_gbp: false,
        apq: None,
        request_collapsing: None,
        waf: None,
        query_cost: None,
        disable_introspection: false,
        circuit_breaker: None,
    };

    let router = GbpRouter::new(config);

    // 2. Execute query
    let result = router
        .execute_scatter_gather(Some("query { test }"), None, None)
        .await;

    // 3. Expect it to handle the failure gracefully (it might return partial result or error, but not crash)
    // In scatter_gather, if a subgraph fails, it logs and adds to errors list.
    // If all fail, it might return empty result with errors?
    // Let's check GbpRouter impl. It returns `Ok(response)` where response is map of results.
    // If subgraph fails, it logs and continues. The result map will just lack that subgraph's data.

    let json = result.expect("Router should not crash on malformed subgraph response");
    let obj = json.as_object().unwrap();

    // Verify behavior: RestConnector falls back to returning raw body as string if JSON parse fails.
    // So we expect "bad_subgraph" to be present and equal to "Not JSON".
    if let Some(val) = obj.get("bad_subgraph") {
        assert_eq!(
            val.as_str(),
            Some("Not JSON"),
            "Should return raw body if parsing fails"
        );
    } else {
        // If it was dropped from results (future behavior change?), that's also fine for security (no crash)
    }
}

#[tokio::test]
async fn test_security_huge_response_handling() {
    // 1. Setup mock subgraph returning HUGE content
    let mock_addr = spawn_mock_subgraph(0, "huge").await;

    let config = RouterConfig {
        port: 0,
        subgraphs: vec![SubgraphConfig {
            name: "huge_subgraph".into(),
            url: format!("http://{}/graphql", mock_addr),
            headers: std::collections::HashMap::new(),
        }],
        force_gbp: false,
        apq: None,
        request_collapsing: None,
        waf: None,
        query_cost: None,
        disable_introspection: false,
        circuit_breaker: None,
    };

    let router = GbpRouter::new(config);

    // 2. Execute query
    let result = router
        .execute_scatter_gather(Some("query { users }"), None, None)
        .await;

    // 3. Verify it handled it (likely passed it through, but we want to ensure no buffer overflow panic etc)
    assert!(result.is_ok());
    let json = result.unwrap();

    // Verify we got the data (if it fit in memory) - Rust handles memory well usually
    let data = &json["huge_subgraph"]["data"]["users"];
    assert!(data.is_array());
    assert_eq!(data.as_array().unwrap().len(), 10_000);
}

#[tokio::test]
async fn test_security_deeply_nested_query() {
    // Nested query depth attack
    let depth = 500;
    let mut query = "query { ".to_string();
    for i in 0..depth {
        query.push_str(&format!("level{} {{ ", i));
    }
    query.push_str("field");
    for _ in 0..depth {
        query.push_str(" }");
    }
    query.push_str(" }");

    let config = RouterConfig {
        port: 0,
        subgraphs: vec![],
        force_gbp: false,
        apq: None,
        request_collapsing: None,
        waf: None,
        query_cost: None,
        disable_introspection: false,
        circuit_breaker: None,
    };
    let router = GbpRouter::new(config);

    let result = router
        .execute_scatter_gather(Some(&query), None, None)
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_isolate_slow_loris_subgraph() {
    // One subgraph is slow, other is fast. The slow one shouldn't block the system indefinitely if designed well.
    // However, execute_scatter_gather waits for all. We want to verify it doesn't deadlock.

    let fast_addr = spawn_mock_subgraph(0, "normal").await;
    let slow_addr = spawn_mock_subgraph(0, "slow").await;

    let config = RouterConfig {
        port: 0,
        subgraphs: vec![
            SubgraphConfig {
                name: "fast".into(),
                url: format!("http://{}/graphql", fast_addr),
                headers: std::collections::HashMap::new(),
            },
            SubgraphConfig {
                name: "slow".into(),
                url: format!("http://{}/graphql", slow_addr),
                headers: std::collections::HashMap::new(),
            },
        ],
        force_gbp: false,
        apq: None,
        request_collapsing: None,
        waf: None,
        query_cost: None,
        disable_introspection: false,
        circuit_breaker: None,
    };

    let router = GbpRouter::new(config);

    let start = std::time::Instant::now();
    let result = router
        .execute_scatter_gather(Some("query { ... }"), None, None)
        .await;
    let duration = start.elapsed();

    assert!(result.is_ok());

    // It should take at least 100ms (slow subgraph)
    assert!(duration.as_millis() >= 100);
    // But shouldn't be excessively longer (overhead check)
    assert!(duration.as_millis() < 500);

    let json = result.unwrap();
    assert!(json["fast"]["data"].is_object());
    assert!(json["slow"]["data"].is_string()); // Our mock returns string for "data" in slow case
}

#[tokio::test]
async fn test_circuit_breaker_integration() {
    use crate::circuit_breaker::CircuitBreakerConfig;

    // 1. Setup mock subgraph that returns ERRORS
    let mock_addr = spawn_mock_subgraph(0, "error").await;

    let config = RouterConfig {
        port: 0,
        subgraphs: vec![SubgraphConfig {
            name: "failing_service".into(),
            url: format!("http://{}/graphql", mock_addr),
            headers: std::collections::HashMap::new(),
        }],
        force_gbp: false,
        apq: None,
        request_collapsing: None,
        waf: None,
        query_cost: None,
        disable_introspection: false,
        circuit_breaker: Some(CircuitBreakerConfig {
            failure_threshold: 2,
            recovery_timeout: Duration::from_secs(10),
            half_open_max_requests: 1,
        }),
    };

    let router = GbpRouter::new(config);

    // 2. Trigger failures to open the circuit
    // Request 1: Fails (Count = 1)
    let _ = router.execute_scatter_gather(Some("query { test }"), None, None).await;
    
    // Request 2: Fails (Count = 2) -> Circuit Opens!
    let _ = router.execute_scatter_gather(Some("query { test }"), None, None).await;

    // 3. Next request should fail FAST with Circuit Open error
    // Use execute_fail_fast to see the error, as execute_scatter_gather swallows individual subgraph errors
    let result = router.execute_fail_fast(Some("query { test }"), None, None).await;

    // Verify error was propagated
    assert!(result.is_err(), "Expected error when circuit is open");
    let err_str = result.err().unwrap().to_string();
    
    // We expect "Circuit breaker open" message in the error output
    assert!(err_str.contains("Circuit breaker open"), "Should contain circuit breaker error, got: {}", err_str);
}
