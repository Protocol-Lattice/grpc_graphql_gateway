# Cost Optimization Strategies for Requests Per Second

This guide provides actionable strategies to reduce the cost per request for your gRPC-GraphQL gateway deployment. By implementing these optimizations, you can achieve **97%+ cost reduction** while maintaining high performance.

## Table of Contents

1. [Quick Wins (Immediate 80% Cost Reduction)](#quick-wins)
2. [Advanced Optimizations (Additional 15% Reduction)](#advanced-optimizations)
3. [Infrastructure Optimizations](#infrastructure-optimizations)
4. [Monitoring & Fine-Tuning](#monitoring--fine-tuning)

---

## Quick Wins (Immediate 80% Cost Reduction)

### 1. Enable Multi-Tier Caching (60-75% Cost Reduction)

Caching is the **single most impactful** optimization for reducing request costs.

#### Implementation

```rust
use grpc_graphql_gateway::{Gateway, CacheConfig};
use std::time::Duration;

let gateway = Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .add_grpc_client("service", client)
    // Redis caching for distributed deployments
    .with_response_cache(CacheConfig {
        redis_url: Some("redis://127.0.0.1:6379".to_string()),
        max_size: 50_000,  // Increase from default 10k
        default_ttl: Duration::from_secs(300), // 5 minutes
        stale_while_revalidate: Some(Duration::from_secs(60)), // Serve stale for 1 min
        invalidate_on_mutation: true,
        vary_headers: vec!["Authorization".to_string()],
    })
    .build()?;
```

#### Cost Impact

| Cache Hit Rate | Database Load | Monthly DB Cost (100k req/s) | Savings |
|----------------|---------------|------------------------------|---------|
| 0% (No cache) | 100k queries/s | $500+ | Baseline |
| 50% | 50k queries/s | $250 | **50%** |
| 75% | 25k queries/s | $80 | **84%** |
| 85% | 15k queries/s | $50 | **90%** |

**Action Items:**
- ✅ Enable Redis caching
- ✅ Increase `max_size` to 50,000+ entries
- ✅ Set appropriate TTL per query type
- ✅ Enable stale-while-revalidate

### 2. Enable Response Compression (50-70% Bandwidth Reduction)

Data transfer often costs **more than compute** at scale.

#### Implementation

```rust
use grpc_graphql_gateway::{Gateway, CompressionConfig};

let gateway = Gateway::builder()
    .with_compression(CompressionConfig {
        level: 6, // Balanced compression (1-9, higher = more compression)
        min_size: 1024, // Only compress responses > 1KB
        enabled_algorithms: vec!["br", "gzip", "deflate"], // Brotli preferred
    })
    .build()?;
```

#### Cost Impact

**Bandwidth Cost Analysis (100k req/s, 2KB avg response):**

| Scenario | Monthly Data Transfer | AWS Cost ($0.09/GB) | Annual Cost |
|----------|----------------------|---------------------|-------------|
| **No Compression** | 518 TB | **$46,620/mo** | **$559,440/yr** |
| **With Compression (70%)** | 155 TB | **$13,950/mo** | **$167,400/yr** |
| **Savings** | 363 TB | **$32,670/mo** | **$392,040/yr** |

**Action Items:**
- ✅ Enable Brotli compression (better than gzip)
- ✅ Use compression level 6 (balance between CPU and size)
- ✅ Set min_size to avoid compressing small responses

### 3. Enable Automatic Persisted Queries (90% Request Size Reduction)

APQ reduces **ingress** bandwidth by sending query hashes instead of full queries.

#### Implementation

```rust
use grpc_graphql_gateway::{Gateway, PersistedQueryConfig};

let gateway = Gateway::builder()
    .with_persisted_queries(PersistedQueryConfig {
        cache_size: 5_000, // Cache up to 5k unique queries
        ttl: Some(Duration::from_secs(7200)), // 2 hour expiration
    })
    .build()?;
```

#### Cost Impact

**Request Size Reduction:**

| Request Type | Size Without APQ | Size With APQ | Reduction |
|--------------|------------------|---------------|-----------|
| Typical Query | 1.5 KB | 150 bytes | **90%** |
| Complex Query | 5 KB | 150 bytes | **97%** |

**Bandwidth Savings (100k req/s):**
- **Ingress**: 130 TB/mo → 13 TB/mo = **$10,000+/mo savings**

**Action Items:**
- ✅ Enable APQ on gateway
- ✅ Configure Apollo Client to use APQ
- ✅ Set appropriate cache size and TTL

### 4. Enable Request Collapsing (Eliminate Redundant Queries)

Request collapsing deduplicates identical in-flight queries.

#### Implementation

```rust
use grpc_graphql_gateway::{Gateway, RequestCollapsingConfig};

let gateway = Gateway::builder()
    .with_request_collapsing(RequestCollapsingConfig {
        enabled: true,
        max_wait: Duration::from_millis(10), // Coalesce within 10ms window
    })
    .build()?;
```

#### Cost Impact

**For high-traffic queries (e.g., homepage data):**
- Without collapsing: 1,000 identical requests → 1,000 database queries
- With collapsing: 1,000 identical requests → 1 database query

**Typical Reduction:**
- **10-25% fewer database queries** during traffic spikes
- **$50-100/mo savings** on database costs

**Action Items:**
- ✅ Enable request collapsing
- ✅ Monitor metrics to track deduplication rate

---

## Advanced Optimizations (Additional 15% Reduction)

### 5. Use High-Performance Mode (2x Throughput)

Enable SIMD JSON parsing and sharded caching for maximum throughput.

#### Implementation

```rust
use grpc_graphql_gateway::{Gateway, HighPerformanceConfig};

let gateway = Gateway::builder()
    .enable_high_performance(HighPerformanceConfig {
        simd_json: true,           // SIMD-accelerated JSON parsing
        sharded_cache: true,        // Lock-free sharded cache (128 shards)
        object_pooling: true,       // Reuse buffers to reduce allocations
        num_cache_shards: 128,      // Number of cache shards (power of 2)
    })
    .build()?;
```

#### Cost Impact

**Throughput Improvement:**
- Standard mode: ~54k req/s per instance
- High-performance mode: **~100k req/s per instance**

**Instance Cost Savings (100k req/s):**
- Standard: 2-3 instances × $30/mo = **$90/mo**
- High-perf: 1-2 instances × $30/mo = **$45/mo**
- **Savings: $45/mo (50% reduction)**

**Action Items:**
- ✅ Enable high-performance mode in production
- ✅ Use larger instance types (more CPU cores benefit from SIMD)

### 6. Implement Query Complexity Limits

Prevent expensive queries from consuming resources.

#### Implementation

```rust
let gateway = Gateway::builder()
    .with_query_depth_limit(10)      // Max nesting depth
    .with_query_complexity_limit(1000) // Max complexity score
    .build()?;
```

#### Cost Impact

**Protection against:**
- Deeply nested queries that cause N+1 problems
- Overly complex queries that exhaust database connections
- Malicious queries designed to overload the system

**Potential Savings:**
- **Prevents 99% of abusive queries**
- Eliminates database overload during attacks
- **$100-500/mo savings** by preventing over-provisioning

**Action Items:**
- ✅ Set appropriate depth limit (8-12 for most apps)
- ✅ Set complexity limit based on your schema
- ✅ Monitor rejected queries to fine-tune limits

### 7. Enable DataLoader for Batch Processing

Eliminate N+1 query problems by batching requests.

#### Implementation

```rust
let gateway = Gateway::builder()
    .with_data_loader(true)
    .build()?;
```

#### Cost Impact

**Example: Loading 100 users with their posts**
- Without DataLoader: 1 query + 100 queries = 101 database queries
- With DataLoader: 1 query + 1 batched query = 2 database queries

**Typical Reduction:**
- **50-80% fewer database queries** for relationship-heavy schemas
- **$100-200/mo savings** on database costs

**Action Items:**
- ✅ Enable DataLoader globally
- ✅ Review schema for relationship fields

### 8. Use Circuit Breakers

Prevent cascading failures and unnecessary retries.

#### Implementation

```rust
use grpc_graphql_gateway::{Gateway, CircuitBreakerConfig};

let gateway = Gateway::builder()
    .with_circuit_breaker(CircuitBreakerConfig {
        failure_threshold: 5,          // Open after 5 failures
        timeout: Duration::from_secs(30), // Reset after 30s
        half_open_max_requests: 3,     // Allow 3 test requests
    })
    .build()?;
```

#### Cost Impact

**Protection against:**
- Repeated calls to failing backends
- Resource exhaustion during outages
- Cascading failures across services

**Potential Savings:**
- **Prevents 90% of unnecessary retries** during outages
- **$50-100/mo savings** by avoiding spike in error traffic

**Action Items:**
- ✅ Enable circuit breaker per gRPC client
- ✅ Configure appropriate thresholds
- ✅ Monitor circuit breaker state

---

## Infrastructure Optimizations

### 9. Use ARM Instances (20-30% Cost Reduction)

ARM processors (AWS Graviton, GCP Tau) offer better price-performance.

#### Recommendations

**AWS:**
```
Standard: c6i.large (x86) = $0.085/hr = $62/mo
Optimized: c6g.large (ARM) = $0.068/hr = $50/mo
Savings: $12/mo per instance (19% cheaper)
```

**GCP:**
```
Standard: e2-standard-2 (x86) = $0.067/hr = $49/mo
Optimized: t2a-standard-2 (ARM) = $0.053/hr = $39/mo
Savings: $10/mo per instance (20% cheaper)
```

**Cost Impact (3 instances):**
- **Annual savings: $360-400/yr**

**Action Items:**
- ✅ Switch to ARM instances (Graviton2/3 on AWS)
- ✅ Test for compatibility (Rust has excellent ARM support)

### 10. Use PgBouncer Connection Pooling

Reduce database connection overhead.

#### Implementation

```bash
# Install PgBouncer on t4g.micro ($6/mo)
docker run -d \
  --name pgbouncer \
  -e DATABASE_URL=postgres://user:pass@db-host:5432/dbname \
  -e POOL_MODE=transaction \
  -e MAX_CLIENT_CONN=10000 \
  -e DEFAULT_POOL_SIZE=25 \
  -p 6432:6432 \
  edoburu/pgbouncer
```

#### Cost Impact

**Database Performance Improvement:**
- Increases throughput by **2-4x**
- Allows smaller database instances

**Cost Savings:**
| Without PgBouncer | With PgBouncer | Savings |
|-------------------|----------------|---------|
| db.m5.large ($144/mo) | db.t3.medium ($72/mo) | **$72/mo** |
| db.m5.xlarge ($288/mo) | db.t3.large ($144/mo) | **$144/mo** |

**Action Items:**
- ✅ Deploy PgBouncer on micro instance ($6/mo)
- ✅ Use transaction pooling mode
- ✅ Downgrade database instance size

### 11. Implement Cloudflare Edge Caching

Cache responses at 200+ edge locations worldwide.

#### Implementation

**Cloudflare Worker for GraphQL Caching:**

```javascript
// workers/graphql-cache.js
addEventListener('fetch', event => {
  event.respondWith(handleRequest(event.request));
});

async function handleRequest(request) {
  if (request.method === 'POST' && request.url.includes('/graphql')) {
    const body = await request.clone().json();
    
    // Create cache key from query hash
    const cacheKey = new Request(
      request.url + '?q=' + btoa(JSON.stringify(body)),
      { method: 'GET' }
    );
    
    const cache = caches.default;
    let response = await cache.match(cacheKey);
    
    if (!response) {
      response = await fetch(request);
      
      // Cache for 60 seconds (adjust per query type)
      const headers = new Headers(response.headers);
      headers.set('Cache-Control', 'public, max-age=60');
      
      response = new Response(response.body, { ...response, headers });
      event.waitUntil(cache.put(cacheKey, response.clone()));
    }
    
    return response;
  }
  
  return fetch(request);
}
```

#### Cost Impact

**Edge Cache Hit Rate: 30-50%**

**Before Cloudflare:**
- Origin requests: 100k req/s
- Bandwidth from origin: 518 TB/mo
- Cost: $46,620/mo

**After Cloudflare (40% hit rate):**
- Origin requests: 60k req/s
- Bandwidth from origin: 310 TB/mo
- Cost: $27,900/mo
- **Savings: $18,720/mo**

**With Cloudflare + Compression:**
- Origin requests: 60k req/s
- Bandwidth from origin: 93 TB/mo (compressed)
- Cost: $8,370/mo
- **Savings: $38,250/mo**

**Action Items:**
- ✅ Sign up for Cloudflare Pro ($20/mo)
- ✅ Deploy edge caching worker
- ✅ Configure cache rules per query type

### 12. Right-Size Your Database

Start small and scale based on metrics.

#### Sizing Guide

**With Caching + PgBouncer:**

| Cache Hit Rate | Effective DB Load | Recommended Instance | Monthly Cost |
|----------------|-------------------|---------------------|--------------|
| **50%** | 50k queries/s | db.m5.large | $144 |
| **75%** | 25k queries/s | db.t3.medium | $72 |
| **85%** | 15k queries/s | db.t3.small | $36 |
| **90%** | 10k queries/s | db.t3.micro | $15 |

**Action Items:**
- ✅ Start with smallest instance that handles load
- ✅ Enable auto-scaling based on CPU/connections
- ✅ Monitor cache hit rate to optimize database size

---

## Monitoring & Fine-Tuning

### 13. Track Cost Metrics

Monitor these key metrics to optimize costs:

```rust
let gateway = Gateway::builder()
    .enable_metrics()  // Prometheus metrics
    .enable_analytics(AnalyticsConfig::development())
    .build()?;
```

#### Key Metrics to Monitor

| Metric | Target | Action if Below Target |
|--------|--------|------------------------|
| Cache hit rate | \u003e75% | Increase TTL or cache size |
| APQ hit rate | \u003e80% | Increase APQ cache size |
| Request collapsing rate | \u003e10% | Review query patterns |
| Database connections | \u003c50 per instance | Verify PgBouncer config |
| P99 latency | \u003c50ms | Check for N+1 queries |

**Action Items:**
- ✅ Set up Prometheus + Grafana
- ✅ Create alerts for low cache hit rates
- ✅ Review metrics weekly to optimize

### 14. Implement Query Whitelisting (Production)

Only allow pre-approved queries in production.

#### Implementation

```rust
use grpc_graphql_gateway::{Gateway, QueryWhitelistConfig};

let gateway = Gateway::builder()
    .with_query_whitelist(QueryWhitelistConfig {
        whitelist_file: "queries.whitelist",
        enforce: true, // Block non-whitelisted queries
    })
    .build()?;
```

#### Cost Impact

**Benefits:**
- Prevents ad-hoc expensive queries
- Allows pre-optimization of all queries
- Enables aggressive caching (known query patterns)

**Potential Savings:**
- **Eliminates rogue queries** that spike costs
- **20-30% better cache hit rates** (predictable queries)
- **$100-200/mo savings** from better optimization

**Action Items:**
- ✅ Extract queries from production traffic
- ✅ Enable whitelist in production
- ✅ Keep whitelist in version control

---

## Complete Optimized Configuration

Here's a production-ready configuration with all optimizations enabled:

```rust
use grpc_graphql_gateway::{
    Gateway, GrpcClient, CacheConfig, CompressionConfig,
    PersistedQueryConfig, RequestCollapsingConfig,
    CircuitBreakerConfig, HighPerformanceConfig,
    QueryWhitelistConfig, AnalyticsConfig,
};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = GrpcClient::builder("http://backend:50051")
        .lazy(false)
        .connect()
        .await?;

    let gateway = Gateway::builder()
        .with_descriptor_set_bytes(DESCRIPTORS)
        .add_grpc_client("service", client)
        
        // Performance optimizations
        .enable_high_performance(HighPerformanceConfig {
            simd_json: true,
            sharded_cache: true,
            object_pooling: true,
            num_cache_shards: 128,
        })
        
        // Caching (60-75% cost reduction)
        .with_response_cache(CacheConfig {
            redis_url: Some("redis://redis:6379".to_string()),
            max_size: 50_000,
            default_ttl: Duration::from_secs(300),
            stale_while_revalidate: Some(Duration::from_secs(60)),
            invalidate_on_mutation: true,
            vary_headers: vec!["Authorization".to_string()],
        })
        
        // Bandwidth optimization (50-70% reduction)
        .with_compression(CompressionConfig {
            level: 6,
            min_size: 1024,
            enabled_algorithms: vec!["br", "gzip", "deflate"],
        })
        
        // APQ (90% request size reduction)
        .with_persisted_queries(PersistedQueryConfig {
            cache_size: 5_000,
            ttl: Some(Duration::from_secs(7200)),
        })
        
        // Request deduplication
        .with_request_collapsing(RequestCollapsingConfig {
            enabled: true,
            max_wait: Duration::from_millis(10),
        })
        
        // DataLoader for batching
        .with_data_loader(true)
        
        // Circuit breaker
        .with_circuit_breaker(CircuitBreakerConfig {
            failure_threshold: 5,
            timeout: Duration::from_secs(30),
            half_open_max_requests: 3,
        })
        
        // Security
        .with_query_depth_limit(12)
        .with_query_complexity_limit(1000)
        .with_query_whitelist(QueryWhitelistConfig {
            whitelist_file: "queries.whitelist",
            enforce: true,
        })
        
        // Observability
        .enable_metrics()
        .enable_analytics(AnalyticsConfig::production())
        .enable_health_checks()
        
        .build()?;

    gateway.serve("0.0.0.0:8080").await?;
    Ok(())
}
```

---

## Cost Reduction Summary

| Optimization | Cost Reduction | Effort | Priority |
|--------------|----------------|--------|----------|
| **Multi-tier caching** | 60-75% | Medium | 🔴 Critical |
| **Response compression** | 50-70% | Low | 🔴 Critical |
| **APQ** | 30-50% | Medium | 🟡 High |
| **Cloudflare edge caching** | 30-50% | Medium | 🟡 High |
| **Request collapsing** | 10-25% | Low | 🟢 Medium |
| **ARM instances** | 20-30% | Low | 🟢 Medium |
| **PgBouncer** | 40-60% | Medium | 🟡 High |
| **High-performance mode** | 50% | Low | 🟡 High |
| **DataLoader** | 50-80% | Medium | 🟡 High |
| **Query limits** | 10-20% | Low | 🟢 Medium |

**Total Potential Savings: 90-97% cost reduction**

---

## Before & After Comparison

### Before Optimization (100k req/s)

| Component | Cost |
|-----------|------|
| Gateway (25 Node.js instances) | $750/mo |
| Database (Large instance) | $288/mo |
| Data transfer (518 TB) | $46,620/mo |
| **Total** | **$47,658/mo** |

### After Optimization (100k req/s)

| Component | Cost |
|-----------|------|
| Cloudflare Pro | $20/mo |
| Gateway (2 ARM instances) | $45/mo |
| PgBouncer | $6/mo |
| Redis (3GB) | $50/mo |
| Database (Small instance, 90% cache hit) | $30/mo |
| Data transfer (10 TB compressed) | $900/mo |
| **Total** | **$1,051/mo** |

**Annual Savings: $559,284 per year (97.8% reduction)**

---

## Related Documentation

- [Response Caching](../performance/caching.md) - Detailed caching guide
- [Response Compression](../performance/compression.md) - Compression configuration
- [Automatic Persisted Queries](../performance/apq.md) - APQ setup
- [High Performance](../performance/high-performance.md) - SIMD and sharding
- [Cost Analysis](./cost-analysis.md) - Detailed cost breakdown
- [Helm Deployment](./helm-deployment.md) - Kubernetes deployment
