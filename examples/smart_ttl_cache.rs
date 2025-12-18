// Example: Using Smart TTL with the Response Cache
//
// This example demonstrates how to integrate Smart TTL Management
// with the response cache for intelligent cache duration optimization.

use grpc_graphql_gateway::{CacheConfig, Gateway, GrpcClient, SmartTtlConfig, SmartTtlManager};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Configure Smart TTL with custom patterns
    let mut custom_patterns = HashMap::new();
    custom_patterns.insert("dashboard".to_string(), Duration::from_secs(60));
    custom_patterns.insert("reports".to_string(), Duration::from_secs(1800));
    custom_patterns.insert("search".to_string(), Duration::from_secs(300));

    let smart_ttl_config = SmartTtlConfig {
        // Base TTLs for different query types
        default_ttl: Duration::from_secs(300),      // 5 minutes
        user_profile_ttl: Duration::from_secs(900), // 15 minutes
        static_content_ttl: Duration::from_secs(86400), // 24 hours
        real_time_data_ttl: Duration::from_secs(5), // 5 seconds
        aggregated_data_ttl: Duration::from_secs(1800), // 30 minutes
        list_query_ttl: Duration::from_secs(600),   // 10 minutes
        item_query_ttl: Duration::from_secs(300),   // 5 minutes

        // Learning configuration
        auto_detect_volatility: true,
        min_observations: 10,       // Learn after 10 executions
        max_adjustment_factor: 2.0, // Can double or halve TTL

        // Custom patterns
        custom_patterns,

        // Honor schema hints
        respect_cache_hints: true,
    };

    // 2. Create Smart TTL Manager
    let smart_ttl = Arc::new(SmartTtlManager::new(smart_ttl_config));

    // 3. Configure cache with Smart TTL
    let cache_config = CacheConfig {
        max_size: 50_000,
        default_ttl: Duration::from_secs(300), // Fallback
        stale_while_revalidate: Some(Duration::from_secs(60)),
        invalidate_on_mutation: true,
        redis_url: Some("redis://127.0.0.1:6379".to_string()),
        vary_headers: vec!["Authorization".to_string()],
        smart_ttl_manager: Some(smart_ttl.clone()), // Enable Smart TTL!
    };

    // 4. Build the gateway
    const DESCRIPTORS: &[u8] = include_bytes!("../src/generated/greeter_descriptor.bin");

    let grpc_client = GrpcClient::builder("http://localhost:50051").connect_lazy()?;

    let gateway = Gateway::builder()
        .with_descriptor_set_bytes(DESCRIPTORS)
        .add_grpc_client("service", grpc_client)
        .with_response_cache(cache_config)
        .build()?;

    // 5. Start background task to monitor Smart TTL effectiveness
    tokio::spawn({
        let smart_ttl = smart_ttl.clone();
        async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;

                let analytics = smart_ttl.get_analytics().await;
                tracing::info!(
                    total_queries = analytics.total_queries,
                    avg_volatility = %format!("{:.2}%", analytics.avg_volatility_score * 100.0),
                    avg_ttl_secs = analytics.avg_recommended_ttl.as_secs(),
                    volatile_queries = analytics.highly_volatile_queries,
                    stable_queries = analytics.stable_queries,
                    "Smart TTL Analytics"
                );

                // Cleanup old stats
                smart_ttl
                    .cleanup_old_stats(Duration::from_secs(86400))
                    .await;
            }
        }
    });

    // 6. Serve the gateway
    println!("🚀 Gateway running with Smart TTL on http://localhost:8888/graphql");
    println!("📊 Smart TTL will automatically optimize cache durations");
    println!("   - Static content: 24 hours");
    println!("   - User profiles: 15 minutes");
    println!("   - Real-time data: 5 seconds");
    println!("   - Learning enabled: TTLs will auto-adjust based on data volatility");

    let app = gateway.into_router();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8888").await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// Example of using the cache directly with Smart TTL
async fn example_cache_usage(
    cache: &grpc_graphql_gateway::ResponseCache,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::collections::HashSet;

    let query = r#"
        query GetUser($id: ID!) {
            user(id: $id) {
                id
                name
                email
            }
        }
    "#;

    let cache_key = "user:123".to_string();
    let response = serde_json::json!({
        "data": {
            "user": {
                "id": "123",
                "name": "Alice",
                "email": "alice@example.com"
            }
        }
    });

    // Use put_with_query to enable Smart TTL calculation
    cache
        .put_with_query(
            cache_key,
            query,
            "user", // Query type for TTL calculation
            response,
            HashSet::from(["User".to_string()]),
            HashSet::from(["User#123".to_string()]),
        )
        .await;

    Ok(())
}
