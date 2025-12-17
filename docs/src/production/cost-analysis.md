# Cost Analysis

This guide provides a comprehensive cost analysis for running **grpc_graphql_gateway** in production environments, with specific calculations for handling **100,000 requests per second**.

## Performance Baseline

Based on our benchmarks, grpc_graphql_gateway achieves:

| Metric | Value |
|--------|-------|
| **Single instance throughput** | ~54,000 req/s |
| **Comparison to Apollo Server** | 27x faster |
| **Memory footprint** | 100-200MB per instance |

To handle **100k req/s**, you need approximately **2-3 instances** (with headroom for spikes).

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        CLOUDFLARE PRO                                   │
│  ┌─────────────────────────────────────────────────────────────────┐   │
│  │  Edge Cache (200+ PoPs worldwide)                                │   │
│  │  • GraphQL response caching                                      │   │
│  │  • DDoS protection                                               │   │
│  │  • WAF rules                                                     │   │
│  └─────────────────────────┬───────────────────────────────────────┘   │
└────────────────────────────┼────────────────────────────────────────────┘
                             │ Cache MISS
                             ▼
                    ┌─────────────────┐
                    │  Load Balancer  │
                    └────────┬────────┘
                             │
         ┌───────────────────┼───────────────────┐
         ▼                   ▼                   ▼
    ┌─────────┐         ┌─────────┐         ┌─────────┐
    │ Gateway │         │ Gateway │         │ Gateway │
    │   #1    │         │   #2    │         │   #3    │
    └────┬────┘         └────┬────┘         └────┬────┘
         │                   │                   │
         └───────────────────┼───────────────────┘
                             │
              ┌──────────────┴──────────────┐
              ▼                             ▼
      ┌───────────────┐            ┌────────────────┐
      │  Redis Cache  │            │  gRPC Services │
      │   (L2 Cache)  │            │                │
      └───────────────┘            └───────┬────────┘
                                           │
                                           ▼
                                   ┌───────────────┐
                                   │   Database    │
                                   │ (PostgreSQL)  │
                                   └───────────────┘
```

---

## Cloud Provider Cost Estimates

### AWS Stack

| Component | Specification | Monthly Cost |
|-----------|--------------|--------------|
| **Cloudflare Pro** | Pro Plan + Cache API | $20 |
| **Gateway Instances** | 3× `c6g.large` (2 vCPU, 4GB ARM) | $90 |
| **Load Balancer** | ALB | $22 |
| **Redis (L2 Cache)** | ElastiCache `cache.t3.medium` (3GB) | $50 |
| **PostgreSQL (HA)** | RDS `db.t3.medium` (Multi-AZ) | $140 |
| **PostgreSQL (Basic)** | RDS `db.t3.small` (Single-AZ) | $30 |
| **Data Transfer** | ~500GB egress (estimated) | $45 |
| | | |
| **Total (Production HA)** | With Multi-AZ DB | **~$370/month** |
| **Total (Cost-Optimized)** | Single-AZ DB | **~$260/month** |

### GCP Stack

| Component | Specification | Monthly Cost |
|-----------|--------------|--------------|
| **Cloudflare Pro** | Pro Plan | $20 |
| **Gateway Instances** | 3× `e2-standard-2` | $75 |
| **Load Balancer** | Cloud Load Balancing | $20 |
| **Redis (L2 Cache)** | Memorystore 3GB | $55 |
| **PostgreSQL (HA)** | Cloud SQL `db-custom-2-4096` (HA) | $120 |
| **PostgreSQL (Basic)** | Cloud SQL `db-f1-micro` | $10 |
| **Data Transfer** | ~500GB egress | $40 |
| | | |
| **Total (Production HA)** | With HA database | **~$330/month** |
| **Total (Cost-Optimized)** | Basic database | **~$220/month** |

### Azure Stack

| Component | Specification | Monthly Cost |
|-----------|--------------|--------------|
| **Cloudflare Pro** | Pro Plan | $20 |
| **Gateway Instances** | 3× `Standard_D2s_v3` | $105 |
| **Load Balancer** | Standard LB | $25 |
| **Redis (L2 Cache)** | Azure Cache 3GB | $55 |
| **PostgreSQL (HA)** | Flexible Server (Zone Redundant) | $150 |
| **Data Transfer** | ~500GB egress | $45 |
| | | |
| **Total (Production HA)** | | **~$400/month** |

---

## Cloudflare Pro Benefits

| Feature | Benefit |
|---------|---------|
| **Edge Caching** | Cache GraphQL responses at 200+ edge locations |
| **Cache Rules** | Custom caching for `POST /graphql` with query hash |
| **WAF** | Block malicious GraphQL queries |
| **Rate Limiting** | 10 rules included, protect per-endpoint |
| **Analytics** | Real-time traffic insights |
| **DDoS Protection** | Layer 3/4/7 protection included |

### GraphQL Edge Caching with Cloudflare Workers

```javascript
// workers/graphql-cache.js
addEventListener('fetch', event => {
  event.respondWith(handleRequest(event.request));
});

async function handleRequest(request) {
  if (request.method === 'POST') {
    const body = await request.clone().json();
    
    // Create cache key from query + variables
    const cacheKey = new Request(
      request.url + '?q=' + btoa(JSON.stringify(body)),
      { method: 'GET' }
    );
    
    const cache = caches.default;
    let response = await cache.match(cacheKey);
    
    if (!response) {
      response = await fetch(request);
      
      // Cache for 60 seconds
      const headers = new Headers(response.headers);
      headers.set('Cache-Control', 'max-age=60');
      
      response = new Response(response.body, { ...response, headers });
      event.waitUntil(cache.put(cacheKey, response.clone()));
    }
    
    return response;
  }
  
  return fetch(request);
}
```

---

## 3-Tier Caching Strategy

Implementing a multi-tier caching strategy significantly reduces costs by minimizing database load:

```
Request Flow:
                                   Cache Hit Rate
┌─────────────┐                    ─────────────
│ Cloudflare  │ ──── HIT (40%) ──→ Response    ← Edge, <10ms
│ Edge Cache  │
└──────┬──────┘
       │ MISS
       ▼
┌─────────────┐
│   Gateway   │
│ Redis Cache │ ──── HIT (35%) ──→ Response    ← L2, 1-5ms
└──────┬──────┘
       │ MISS
       ▼
┌─────────────┐
│   Database  │ ──── Query (25%) → Response    ← Origin, 5-50ms
└─────────────┘

Total cache hit rate: ~75%
Database load reduced by: 75%
```

### Gateway Configuration for Caching

```rust
use grpc_graphql_gateway::{Gateway, CacheConfig};

Gateway::builder()
    .with_descriptor_set_bytes(DESCRIPTORS)
    .add_grpc_client("service", client)
    // Enable Redis caching
    .with_response_cache(CacheConfig::builder()
        .redis_url("redis://localhost:6379")
        .default_ttl(Duration::from_secs(300))
        .build())
    // Enable DataLoader for batching
    .with_data_loader(true)
    // Protection
    .with_rate_limiter(RateLimiterConfig::new(150_000))
    .with_circuit_breaker(CircuitBreakerConfig::default())
    // Observability
    .enable_metrics()
    .enable_health_checks()
    .build()?
```

---

## Database Sizing Guide

With proper caching, your database load is significantly reduced:

| Cache Hit Rate | Effective DB Load (for 100k req/s) |
|----------------|-----------------------------------|
| 50% | 50,000 queries/s |
| 75% | 25,000 queries/s |
| 85% | 15,000 queries/s |
| 90% | 10,000 queries/s |

### Bandwidth Cost Analysis (The Hidden Giant)

For **100k req/s**, data transfer is often the largest cost.
*Assumption: 2KB average response size.*

**Total Data Transfer**: `2KB * 100k/s` ≈ **518 TB/month**.

| Scenario | Egress Data | AWS Cost ($0.09/GB) |
|----------|-------------|---------------------|
| **1. Raw Traffic** | 518 TB | **$46,620 / mo** 😱 |
| **2. + Compression (70%)** | 155 TB | **$13,950 / mo** |
| **3. + Cloudflare (80% Hit)** | 31 TB | **$2,790 / mo** |
| **4. + Both** | ~10 TB | **$900 / mo** |

**How to achieve Scenario 4:**
1.  **Compression**: Enable Brotli/Gzip in Gateway (`.with_compression(CompressionConfig::default())`).
2.  **APQ**: Enable Automatic Persisted Queries to reduce Ingress bandwidth.
3.  **Cloudflare**: Cache common queries at the edge.

> **Savings**: Compression and Caching save you over **$45,000/month** in bandwidth costs.

### Database Optimization with PgBouncer

Adding **PgBouncer** (connection pooler) is critical for high-throughput GraphQL workloads. It reduces connection overhead by reusing existing connections, allowing you to handle significantly more requests with smaller database instances.

| Optimization | Impact | Cost Saving |
|--------------|--------|-------------|
| **PgBouncer** | Increases transaction throughput by 2-4x | Downgrade DB tier (e.g., Large → Medium) |
| **Read Replicas** | Offloads read traffic from primary | Scale horizontally instead of vertically |

**Revised Database Sizing with PgBouncer:**

| Database Size | Ops/sec (Raw) | Ops/sec (w/ PgBouncer) | Monthly Cost |
|---------------|---------------|------------------------|--------------|
| **Small** | ~2,000 | ~8,000 | $30-50 |
| **Medium** | ~5,000 | ~25,000 | $100-150 |
| **Large** | ~15,000 | ~60,000+ | $300-500 |

> **Recommendation:** With PgBouncer + Redis Caching, a **Medium** instance or even a well-tuned **Small** instance can often handle 100k req/s traffic if the cache hit rate is high (>85%).

---

## Cost Comparison: grpc_graphql_gateway vs Apollo Server

| Metric | **grpc_graphql_gateway** | **Apollo Server (Node.js)** |
|--------|--------------------------|----------------------------|
| Single instance throughput | ~54,000 req/s | ~4,000 req/s |
| Instances for 100k req/s | **3** | **25-30** |
| Gateway instances cost | **~$90/month** | **~$750/month** |
| Memory per instance | 100-200MB | 512MB-1GB |
| Total monthly cost | **~$370** | **~$1,200+** |
| Annual cost | **~$4,440** | **~$14,400+** |
| **Annual savings** | | **~$10,000** |

### Cost Savings Visualization

```
Apollo Server (25 instances): $$$$$$$$$$$$$$$$$$$$$$$$$
grpc_graphql_gateway (3):     $$$$

Savings: ~92% reduction in gateway costs
```

---

## Pricing Tiers Summary

| Tier | Components | Monthly Cost | Best For |
|------|-----------|--------------|----------|
| **Development** | 1 Gateway + SQLite | **~$20/month** | Local/Dev |
| **Staging** | 2 Gateways + CF Free + Managed DB | **~$100/month** | Staging |
| **Production** | 3 Gateways + CF Pro + Redis + PgBouncer + Postgres | **~$1,200/month** | 100k req/s (Public) |
| **Enterprise** | 5 Gateways + CF Business + Redis Cluster + DB Cluster | **~$2,500+/month** | High Volume |

## Scaling Scenarios

Cost estimates based on user count (assuming 0.5 req/s per active user):

| Metric | Startup (1k Users) | Growth (10k Users) | Scale (100k Users) | High Scale |
|--------|-------------------|--------------------|--------------------|------------|
| **Est. Load** | ~500 req/s | **~5,000 req/s** | ~50,000 req/s | 100k req/s |
| **Gateways** | 1 (`t4g.micro`) | **2 (`t4g.small`)** | 3 (`c6g.medium`) | 3 (`c6g.large`) |
| **Database** | SQLite / Low | **Small RDS** | Medium RDS | Optimized HA |
| **Bandwidth** | Free Tier | **~$50/mo** | ~$450/mo | ~$900/mo |
| **Total Cost** | **~$20 / mo** | **~$155 / mo** | **~$600 / mo** | **~$1,200 / mo** |

> **Note**: "10k users online" usually generates ~5,000 req/s. At this scale, your infrastructure cost is negligible (<$200) because the gateway is so efficient.

## Profitability Analysis (ROI)

Since your infrastructure cost is so low (~$155/mo for 10k users), you achieve profitability much faster than with traditional stacks.

**Revenue Potential Scaling (Freemium Model):**
*Assumption: 5% of users convert to a $9/mo plan.*

| User Base | Monthly Revenue | Infra Cost (Ops) | **Net Profit** |
|-----------|----------------:|------------------|----------------|
| **1,000** | $450 | ~$20 | **$430** (95% Margin) |
| **10,000** | $4,500 | ~$155 | **$4,345** (96% Margin) |
| **100,000** | **$45,000** | ~$600 | **$44,400** (98% Margin) |
| **1 Million** | **$450,000** | ~$6,000 | **$444,000** (98% Margin) |

> **The "Rust Scaling Advantage"**:
> With Node.js or Java, your infrastructure costs usually grow *linearly* with users ($20 -> $200 -> $2,000).
> With this optimized Rust stack, your costs grow *sub-linearly* thanks to high efficiency, meaning **your profit margins actually increase as you scale**.

---

## Quick Reference Card

```
┌─────────────────────────────────────────────────────────┐
│  100k req/s Full Stack - Optimized                      │
├─────────────────────────────────────────────────────────┤
│  Cloudflare Pro .......................... $20/month   │
│  3× Gateway (c6g.large) .................. $90/month   │
│  PgBouncer (t4g.micro) ................... $10/month   │
│  Redis 3GB ............................... $50/month   │
│  PostgreSQL (Optimization) ............... $80/month   │
│  Data Transfer (Optimized 10TB) .......... $900/month  │
├─────────────────────────────────────────────────────────┤
│  TOTAL .................................. ~$1,150/month │
│  Annual ................................ ~$13,800/year  │
│  vs Unoptimized (~$47k/mo) ............. save $500k/yr │
└─────────────────────────────────────────────────────────┘
```

---

## Cost Optimization Tips

1. **Use PgBouncer** - Essential for high concurrency.
2. **Use ARM instances** (`c6g` on AWS, `t2a` on GCP) - 20% cheaper than x86.
3. **Enable response caching** - Reduces backend load by 60-80%.
4. **Bandwidth Optimization** - Use [APQ](../performance/apq.md) and [Compression](../performance/compression.md) to cut data transfer costs by 50-90%.
5. **Use Cloudflare edge caching** - Reduces origin requests by 30-50%
4. **Right-size your database** - Start small, scale based on metrics
5. **Use Reserved Instances** - Save 30-60% on long-term commitments
6. **Enable compression** - Reduces data transfer costs

---

## Next Steps

- [Helm Deployment](./helm-deployment.md) - Deploy to Kubernetes
- [Autoscaling](./autoscaling.md) - Configure horizontal pod autoscaling
- [Response Caching](../performance/caching.md) - Configure Redis caching
