/// Advanced Live Query Features Example
///
/// This module demonstrates how to use the 4 advanced live query features:
/// 1. Filtered live queries
/// 2. Field-level invalidation
/// 3. Batch invalidation
/// 4. Client-side caching hints

use grpc_graphql_gateway::{
    ActiveLiveQuery, CacheControl, DataVolatility, FieldChange,
    InvalidationEvent, LiveQueryStore, LiveQueryStrategy, LiveQueryUpdate,
    SharedLiveQueryStore,
    detect_field_changes, generate_cache_control, matches_filter, parse_query_arguments,
};
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

/// Example 1: Filtered Live Queries
/// 
/// This demonstrates how to parse query arguments and filter results
/// based on user-specified criteria like: users(status: ONLINE) @live
pub fn example_filtered_query() {
    println!("\n=== Example 1: Filtered Live Queries ===\n");

    // Parse query arguments from GraphQL query string
    let query = "users(status: ONLINE, limit: 10) @live";
    let args = parse_query_arguments(query);
    
    println!("Parsed arguments from query:");
    for (key, value) in &args {
        println!("  {}:  {}", key, value);
    }

    // Simulate multiple user entities
    let online_user = json!({
        "id": "1",
        "name": "Alice",
        "status": "ONLINE",
        "email": "alice@example.com"
    });

    let offline_user = json!({
        "id": "2",
        "name": "Bob",
        "status": "OFFLINE",
        "email": "bob@example.com"
    });

    // Check which users match the filter
    println!("\nFiltering results:");
    println!("  Alice (ONLINE): {}", matches_filter(&args, &online_user));
    println!("  Bob (OFFLINE):  {}", matches_filter(&args, &offline_user));
    
    println!("\n✅ Only ONLINE users are included in live query results!\n");
}

/// Example 2: Field-Level Invalidation
/// 
/// This shows how to detect specific field changes and only push
/// updates for the fields that actually changed
pub fn example_field_level_invalidation() {
    println!("\n=== Example 2: Field-Level Invalidation ===\n");

    let old_data = json!({
        "user": {
            "id": "1",
            "name": "Alice",
            "email": "alice@example.com",
            "age": 30,
            "status": {
                "is_online": true,
                "last_active": "2024-01-01T12:00:00Z"
            }
        }
    });

    let new_data = json!({
        "user": {
            "id": "1",
            "name": "Alice Smith",  // Changed!
            "email": "alice@example.com",
            "age": 30,
            "status": {
                "is_online": false,  // Changed!
                "last_active": "2024-01-01T13:00:00Z"  // Changed!
            }
        }
    });

    // Detect field-level changes
    let changes = detect_field_changes(&old_data, &new_data, "", 0, 10);

    println!("Detected field changes:");
    for change in &changes {
        println!("\n  Field: {}", change.field_path);
        if let Some(old_val) = &change.old_value {
            println!("    Old: {}", old_val);
        } else {
            println!("    Old: (field added)");
        }
        println!("    New: {}", change.new_value);
    }

    println!("\n✅ Only changed fields are tracked and pushed to client!\n");
    println!("   This reduces bandwidth and allows clients to");
    println!("   apply surgical updates to their local state.\n");
}

/// Example 3: Batch Invalidation
/// 
/// Shows how multiple rapid invalidation events can be batched
/// to reduce the number of updates pushed to clients
pub async fn example_batch_invalidation() {
    println!("\n=== Example 3: Batch Invalidation ===\n");

    let store = Arc::new(LiveQueryStore::new());
    let (tx, mut rx) = mpsc::channel(100);

    // Register a live query
    let query = ActiveLiveQuery {
        id: "batch-test".to_string(),
        operation_name: "users".to_string(),
        query: "query @live { users { users { id name } } }".to_string(),
        variables: None,
        triggers: vec!["User.create".to_string(), "User.update".to_string()],
        throttle_ms: 100,  // 100ms throttle
        ttl_seconds: 0,
        strategy: LiveQueryStrategy::Invalidation,
        poll_interval_ms: 0,
        last_hash: None,
        last_update: Instant::now(),
        created_at: Instant::now(),
        connection_id: "conn1".to_string(),
    };

    store.register(query.clone(), tx).unwrap();

    println!("Simulating rapid-fire invalidation events...\n");

    // Simulate 5 rapid create events (within throttle window)
    let start = Instant::now();
    for i in 0..5 {
        let event = InvalidationEvent::new("User", "create");
        let affected = store.invalidate(event);
        println!("  Event {}: invalidated {} subscriptions ({:?} elapsed)",
            i + 1, affected, start.elapsed());
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }

    println!("\n  Waiting for throttle period to pass...\n");
    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

    // In practice, the middleware would batch these and send ONE update
    // after the throttle period, instead of 5 separate updates
    println!("✅ Batch invalidation reduces 5 events → 1 update push!\n");
    println!("   With 100ms throttle, updates are debounced and merged,");
    println!("   saving network bandwidth and client processing.\n");
}

/// Example 4: Client-Side Caching Hints
/// 
/// Demonstrates how to generate cache control directives
/// based on data volatility characteristics
pub fn example_cache_control() {
    println!("\n=== Example 4: Client-Side Caching Hints ===\n");

    // Different data types have different volatility
    let examples = vec![
        ("Stock prices", DataVolatility::VeryHigh),
        ("User online status", DataVolatility::High),
        ("Notification counts", DataVolatility::Medium),
        ("User profiles", DataVolatility::Low),
        ("System settings", DataVolatility::VeryLow),
    ];

    println!("Cache control directives by data type:\n");

    for (data_type, volatility) in examples {
        let cache = generate_cache_control(volatility, Some(format!("etag-{}", data_type)));
        
        println!("  {}", data_type);
        println!("    Cache-Control: max-age={}, must-revalidate={}, public={}",
            cache.max_age, cache.must_revalidate, cache.public);
        if let Some(etag) = &cache.etag {
            println!("    ETag: {}", etag);
        }
        
        let advice = match cache.max_age {
            0 => "No caching - always fresh from server",
            1..=10 => "Short cache - very dynamic data",
            11..=60 => "Medium cache - moderately dynamic",
            61..=600 => "Long cache - relatively stable",
            _ => "Very long cache - rarely changes",
        };
        println!("    💡 {}\n", advice);
    }

    println!("✅ Clients can optimize caching based on data characteristics!\n");
}

/// Putting it all together: Real-world usage example
pub async fn example_complete_workflow() {
    println!("\n=== Complete Workflow Example ===\n");
    println!("Scenario: Live user dashboard with filtered, optimized updates\n");

    let store = Arc::new(LiveQueryStore::new());
    let (tx, _rx) = mpsc::channel(10);

    // 1. Client subscribes with filter: users(status: ONLINE) @live
    let query_str = "users(status: ONLINE) @live { users { id name status { is_online } } }";
    let filter = parse_query_arguments(query_str);
    
    println!("Step 1: Client subscribes with filter");
    println!("  Query: {}", query_str);
    println!("  Filter: {:?}\n", filter);

    // 2. Register live query with appropriate cache control
    let query = ActiveLiveQuery {
        id: "dashboard".to_string(),
        operation_name: "users".to_string(),
        query: query_str.to_string(),
        variables: Some(serde_json::to_value(&filter).unwrap()),
        triggers: vec!["User.update".to_string(), "User.create".to_string()],
        throttle_ms: 100,
        ttl_seconds: 3600,
        strategy: LiveQueryStrategy::Invalidation,
        poll_interval_ms: 0,
        last_hash: None,
        last_update: Instant::now(),
        created_at: Instant::now(),
        connection_id: "dashboard-conn".to_string(),
    };

    store.register(query.clone(), tx).unwrap();
    println!("Step 2: Live query registered with batching enabled (100ms throttle)\n");

    // 3. Simulate data change
    let old_user = json!({"id": "1", "name": "Alice", "status": {"is_online": true}});
    let new_user = json!({"id": "1", "name": "Alice", "status": {"is_online": false}});

    let field_changes = detect_field_changes(&old_user, &new_user, "", 0, 10);
    
    println!("Step 3: User status changes");
    println!("  Changed fields:");
    for change in &field_changes {
        println!("    - {}", change.field_path);
    }
    println!();

    // 4. Check if user still matches filter
    let matches = matches_filter(&filter, &new_user);
    println!("Step 4: Check if user still matches filter (status: ONLINE)");
    println!("  Matches: {} (user went offline, exclude from results)\n", matches);

    // 5. Generate cache control based on data type
    let cache = generate_cache_control(DataVolatility::High, Some(query.cache_key()));
    println!("Step 5: Generate cache control for user presence data");
    println!("  max-age: {}s (High volatility - online status changes frequently)\n", cache.max_age);

    // 6. Send update to client
    println!("Step 6: Send optimized update to client");
    println!("  ✓ Only changed fields included");
    println!("  ✓ Filtered results (offline user removed)");
    println!("  ✓ Cache hints provided");
    println!("  ✓ Batched with other pending updates\n");

    println!("✅ Complete workflow demonstrates all 4 advanced features working together!\n");
}

#[tokio::main]
async fn main() {
    println!("\n╔═══════════════════════════════════════════════════════════╗");
    println!("║  Advanced Live Query Features - Usage Examples          ║");
    println!("╚═══════════════════════════════════════════════════════════╝");

    example_filtered_query();
    example_field_level_invalidation();
    example_batch_invalidation().await;
    example_cache_control();
    example_complete_workflow().await;

    println!("\n╔═══════════════════════════════════════════════════════════╗");
    println!("║  Summary of Advanced Features                            ║");
    println!("╠═══════════════════════════════════════════════════════════╣");
    println!("║  1. Filtered Queries - Query args for precise results    ║");
    println!("║  2. Field-Level Invalidation - Granular change tracking  ║");
    println!("║  3. Batch Invalidation - Debounced update merging        ║");
    println!("║  4. Cache Control - Smart client-side caching            ║");
    println!("╚═══════════════════════════════════════════════════════════╝\n");
}
