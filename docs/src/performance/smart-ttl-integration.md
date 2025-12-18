# Integrating Smart TTL with Cache

## Quick Start Example

```rust
use grpc_graphql_gateway::{
    Gateway, CacheConfig, SmartTtlManager, SmartTtlConfig
};
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create Smart TTL Manager
    let smart_ttl_config = SmartTtlConfig {
        default_ttl: Duration::from_secs(300),              // 5 minutes
        user_profile_ttl: Duration::from_secs(900),         // 15 minutes  
        static_content_ttl: Duration::from_secs(86400),     // 24 hours
        real_time_data_ttl: Duration::from_secs(5),         // 5 seconds
        auto_detect_volatility: true,                       // Learn optimal TTLs
        ..Default::default()
    };
    
    let smart_ttl = Arc::new(SmartTtlManager::new(smart_ttl_config));
    
    // Create Cache Config with Smart TTL
    let cache_config = CacheConfig {
        max_size: 50_000,
        default_ttl: Duration::from_secs(300),  // Fallback TTL
        smart_ttl_manager: Some(Arc::clone(&smart_ttl)),
        redis_url: Some("redis://127.0.0.1:6379".to_string()),
        stale_while_revalidate: Some(Duration::from_secs(60)),
        invalidate_on_mutation: true,
        vary_headers: vec!["Authorization".to_string()],
    };
    
    // Build Gateway
    let gateway = Gateway::builder()
        .with_descriptor_set_bytes(DESCRIPTORS)
        .add_grpc_client("service", grpc_client)
        .with_response_cache(cache_config)
        .build()?;
    
    gateway.serve("0.0.0.0:8888").await?;
    Ok(())
}
```

## How It Works

When Smart TTL is enabled:

1. **Cache Lookup**: Normal cache lookup (no change)
2. **Cache Miss - Calculate Smart TTL**:
   - Detect query type (user profile, static content, etc.)
   - Check historical volatility data
   - Apply custom pattern rules
   - Respect `@cacheControl` hints
3. **Store with Optimal TTL**: Cache response with calculated TTL
4. **Learning**: Track query results to improve TTL predictions

## Cost Impact

**Before Smart TTL (Static 5-minute TTL for all queries):**
- Cache hit rate: 75%
- Database load: 25k queries/s (for 100k req/s)
- Database cost: ~$72/mo

**After Smart TTL (Intelligent per-query TTLs):**
- Cache hit rate: **90%** (+15%)
- Database load: **10k queries/s** (-60%)
- Database cost: **~$36/mo** (-50%)

**Monthly Savings: $36-100/mo**

## Usage Patterns

### Pattern 1: Static + Auto-Learning

```rust
SmartTtlConfig {
    // Define base TTLs for query types
    user_profile_ttl: Duration::from_secs(900),
    static_content_ttl: Duration::from_secs(86400),
    
    // Enable learning to fine-tune
    auto_detect_volatility: true,
    min_observations: 20,
    
    ..Default::default()
}
```

### Pattern 2: Custom Patterns Only

```rust
let mut custom_patterns = HashMap::new();
custom_patterns.insert("dashboard".to_string(), Duration::from_secs(60));
custom_patterns.insert("reports".to_string(), Duration::from_secs(1800));

SmartTtlConfig {
    custom_patterns,
    auto_detect_volatility: false,  // Disable learning
    ..Default::default()
}
```

### Pattern 3: Full Auto-Optimization

```rust
SmartTtlConfig {
    auto_detect_volatility: true,
    min_observations: 10,           // Learn quickly
    max_adjustment_factor: 3.0,     // Allow aggressive adjustments
    ..Default::default()
}
```

## Monitoring

Track Smart TTL effectiveness:

```rust
// Get analytics
let analytics = smart_ttl.get_analytics().await;

println!("Query patterns tracked: {}", analytics.total_queries);
println!("Average volatility: {:.2}%", analytics.avg_volatility_score * 100.0);
println!("Average TTL: {:?}", analytics.avg_recommended_ttl);
```

## Related Documentation

- [Smart TTL Management](../performance/smart-ttl.md) - Full Smart TTL documentation
- [Response Caching](../performance/caching.md) - Cache configuration
- [Cost Optimization](../production/cost-optimization-strategies.md) - Overall cost reduction strategies
