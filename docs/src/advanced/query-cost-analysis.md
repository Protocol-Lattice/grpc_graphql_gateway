# Query Cost Analysis

This guide explains how to use the Query Cost Analyzer to track, analyze, and enforce cost budgets for GraphQL queries, preventing expensive queries from spiking infrastructure costs.

## Overview

The Query Cost Analyzer assigns a "cost" to each GraphQL query based on its complexity and enforces budgets at both the query level and per-user level. This prevents expensive queries from overwhelming your infrastructure and helps maintain predictable costs.

## Key Benefits

- **Prevent Cost Spikes**: Block queries that exceed cost thresholds
- **User Budget Enforcement**: Limit query costs per user over time windows
- **Adaptive Cost Multipliers**: Automatically increase costs during high system load
- **Cost Analytics**: Track query costs and identify expensive patterns
- **Database Protection**: Prevent over-provisioning by blocking runaway queries

## Configuration

```rust
use grpc_graphql_gateway::{Gateway, QueryCostConfig};
use std::time::Duration;
use std::collections::HashMap;

let mut field_multipliers = HashMap::new();
field_multipliers.insert("user.posts".to_string(), 50); // 50x cost multiplier
field_multipliers.insert("posts.comments".to_string(), 100); // 100x cost multiplier

let cost_config = QueryCostConfig {
    max_cost_per_query: 1000,        // Reject queries above this cost
    base_cost_per_field: 1,          // Base cost per field
    field_cost_multipliers: field_multipliers,
    user_cost_budget: 10_000,        // Max cost per user per window
    budget_window: Duration::from_secs(60), // 1 minute window
    track_expensive_queries: true,   // Log costly queries
    expensive_percentile: 0.95,      // 95th percentile = "expensive"
    adaptive_costs: true,            // Increase costs during high load
    high_load_multiplier: 2.0,       // 2x cost during peak load
};
```

## Basic Usage

### Calculate Query Cost

```rust
use grpc_graphql_gateway::QueryCostAnalyzer;

let analyzer = QueryCostAnalyzer::new(cost_config);

let query = r#"
    query {
        user(id: 1) {
            id
            name
            posts {
                id
                title
                comments {
                    id
                    text
                }
            }
        }
    }
"#;

// Calculate cost
match analyzer.calculate_query_cost(query).await {
    Ok(result) => {
        println!("Query cost: {}", result.total_cost);
        println!("Field count: {}", result.field_count);
        println!("Complexity: {}", result.complexity);
    }
    Err(e) => {
        println!("Query rejected: {}", e);
        // Return error to client
    }
}
```

### Enforce User Budgets

```rust
let user_id = "user_123";
let query_cost = 250;

// Check if user has budget remaining
match analyzer.check_user_budget(user_id, query_cost).await {
    Ok(()) => {
        // User has budget, execute query
    }
    Err(e) => {
        // User exceeded budget
        println!("Budget exceeded: {}", e);
        // Return rate limit error to client
    }
}
```

## Integration with Gateway

Integrate the cost analyzer into your gateway middleware:

```rust
use grpc_graphql_gateway::{
    Gateway, QueryCostAnalyzer, QueryCostConfig, Middleware,
};
use axum::{
    extract::Extension,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::sync::Arc;

// Create cost analyzer
let cost_analyzer = Arc::new(QueryCostAnalyzer::new(QueryCostConfig::default()));

// Middleware to check query costs
async fn cost_check_middleware(
    Extension(analyzer): Extension<Arc<QueryCostAnalyzer>>,
    query: String,
    user_id: String,
) -> Result<(), Response> {
    // Calculate query cost
    let cost_result = match analyzer.calculate_query_cost(&query).await {
        Ok(result) => result,
        Err(e) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "Query too complex",
                    "message": e,
                })),
            ).into_response());
        }
    };

    // Check user budget
    if let Err(e) = analyzer.check_user_budget(&user_id, cost_result.total_cost).await {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "Budget exceeded",
                "message": e,
            })),
        ).into_response());
    }

    Ok(())
}
```

## Field Cost Multipliers

Assign higher costs to expensive fields:

```rust
let mut multipliers = HashMap::new();

// Relationship fields (can cause N+1 queries)
multipliers.insert("user.posts".to_string(), 50);
multipliers.insert("user.followers".to_string(), 100);
multipliers.insert("post.comments".to_string(), 50);

// Aggregation fields (expensive computations)
multipliers.insert("analytics".to_string(), 200);
multipliers.insert("statistics".to_string(), 150);

// External API calls
multipliers.insert("thirdPartyData".to_string(), 500);

let config = QueryCostConfig {
    field_cost_multipliers: multipliers,
    ..Default::default()
};
```

## Adaptive Cost Multipliers

Automatically increase costs during high system load:

```rust
// Update load factor based on system metrics
let cpu_usage = 0.85; // 85% CPU
let memory_usage = 0.75; // 75% memory

analyzer.update_load_factor(cpu_usage, memory_usage).await;

// Costs will be automatically multiplied by high_load_multiplier
// when average load > 80%
```

## Cost Analytics

Track and analyze query costs:

```rust
// Get analytics
let analytics = analyzer.get_analytics().await;

println!("Total queries tracked: {}", analytics.total_queries);
println!("Average cost: {}", analytics.average_cost);
println!("Median cost: {}", analytics.median_cost);
println!("P95 cost: {}", analytics.p95_cost);
println!("P99 cost: {}", analytics.p99_cost);
println!("Max cost: {}", analytics.max_cost);

// Get threshold for "expensive" queries
let expensive_threshold = analyzer.get_expensive_threshold().await;
println!("Queries above {} are considered expensive", expensive_threshold);
```

## Periodic Cleanup

Clean up expired user budgets to prevent memory growth:

```rust
use tokio::time::{interval, Duration};

// Run cleanup every 5 minutes
let mut cleanup_interval = interval(Duration::from_secs(300));

tokio::spawn({
    let analyzer = Arc::clone(&cost_analyzer);
    async move {
        loop {
            cleanup_interval.tick().await;
            analyzer.cleanup_expired_budgets().await;
        }
    }
});
```

## Cost Optimization Strategies

### 1. Set Appropriate Base Costs

```rust
QueryCostConfig {
    base_cost_per_field: 1,  // Start with 1, adjust based on your schema
    max_cost_per_query: 1000, // Tune based on 95th percentile
    ..Default::default()
}
```

### 2. Identify Expensive Fields

```rust
// Get analytics to find expensive query patterns
let analytics = analyzer.get_analytics().await;

// Queries above P95 should be investigated
if query_cost > analytics.p95_cost {
    // Log for review
    println!("Expensive query detected: cost={}, query={}", query_cost, query);
}
```

### 3. Use Query Whitelisting

Combine with query whitelisting for production:

```rust
// Pre-calculate costs for whitelisted queries
// Reject ad-hoc expensive queries
Gateway::builder()
    .with_query_cost_config(cost_config)
    .with_query_whitelist(whitelist_config)
    .build()?
```

## Cost Impact

By implementing query cost analysis, you can:

| Benefit | Impact |
|---------|--------|
| **Prevent Runaway Queries** | Avoid database overload |
| **Predictable Costs** | No surprise cost spikes |
| **Fair Resource Allocation** | Per-user budgets prevent abuse |
| **Right-Size Infrastructure** | Avoid over-provisioning databases |

**Estimated Monthly Savings**: $200-500 by preventing over-provisioning and database spikes

## Example: E-commerce Schema

```rust
let mut multipliers = HashMap::new();

// Products (relatively cheap)
multipliers.insert("products".to_string(), 1);
multipliers.insert("product.reviews".to_string(), 20);

// Users (moderate cost)
multipliers.insert("user.orders".to_string(), 50);
multipliers.insert("user.wishlist".to_string(), 10);

// Analytics (expensive)
multipliers.insert("salesAnalytics".to_string(), 500);
multipliers.insert("trendingProducts".to_string(), 200);

let config = QueryCostConfig {
    base_cost_per_field: 1,
    max_cost_per_query: 2000,
    field_cost_multipliers: multipliers,
    user_cost_budget: 20_000,
    budget_window: Duration::from_secs(60),
    ..Default::default()
};
```

## Monitoring

Export cost metrics to Prometheus:

```rust
// Add to your metrics collection
gauge!("graphql_query_cost_p95", analytics.p95_cost as f64);
gauge!("graphql_query_cost_p99", analytics.p99_cost as f64);
counter!("graphql_queries_rejected_cost_limit", 1);
counter!("graphql_users_rate_limited_budget", 1);
```

## Best Practices

1. **Start Conservative**: Begin with high limits, then tune down based on analytics
2. **Monitor P95/P99**: Use percentiles to set thresholds, not max values
3. **Whitelist Production Queries**: Pre-approve and optimize expensive queries
4. **Test Under Load**: Verify adaptive cost multipliers work as expected
5. **Budget Windows**: Use 1-minute windows for APIs, 5-minute+ for dashboards

## Related Documentation

- [Cost Optimization Strategies](../production/cost-optimization-strategies.md)
- [Query Whitelisting](../security/query-whitelisting.md)
- [Performance Monitoring](../advanced/metrics.md)
