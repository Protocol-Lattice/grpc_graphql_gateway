# GBP Federation Router: usage "Speed of Light" Design

## 1. Abstract

This document proposes the architecture for a **Native GBP Federation Router**, a specialized mode of the `grpc_graphql_gateway` designed to replace Apollo Router in high-performance environments. By leveraging the **GraphQL Binary Protocol (GBP)** for subgraph communication, this router aims to eliminate the massive serialization/deserialization overhead that plagues standard JSON-based federation, achieving **<1ms overhead per subgraph hop**.

## 2. Generally Accepted Problem

In a standard Federation architecture:
1.  **Client** sends a query to **Router**.
2.  **Router** plans the query and sends HTTP/JSON requests to **Subgraphs**.
3.  **Subgraphs** execute logic, serialize results to JSON strings (CPU heavy), and send back.
4.  **Router** parses JSON strings (CPU heavy), merges results, serializes final JSON, and responds.

**Bottleneck**: **60-80% of CPU time** in a Router/Gateway is spent parsing and serializing JSON. Network bandwidth is also wasted transmitting redundant keys (`"data": {"user": {"id": "...}}`).

## 3. The GBP Solution

We introduce a **Zero-Copy Scatter-Gather Architecture**:

1.  **Router** requests data from Subgraphs with `Accept: application/x-gbp`.
2.  **Subgraphs** (enabled with `grpc_graphql_gateway`) return **GBP Ultra** payloads.
    *   **No Serializing**: Structural hashing replaces string writing.
    *   **No Parsing**: Router decodes binary directly into internal `Value` structures using `GbpDecoder`.
3.  **Router** merges partial results.
    *   Since GBP is structurally aware, merging is often just **pointer linking**.
4.  **Respons**: Router returns GBP (if client supports) or streams JSON.

## 4. Architecture Components

### 4.1. The `FederationRouter` Service
A new top-level component that wraps the `async-graphql` Schema but overrides the default execution strategy.

```rust
pub struct FederationRouter {
    /// The query planner (using Apollo's query planner or a custom one)
    planner: QueryPlanner,
    /// Connection pool to subgraphs, optimized for GBP
    subgraph_clients: HashMap<String, GbpRestClient>,
}
```

### 4.2. `GbpRestClient`
A specialized HTTP client (evolution of `RestConnector`) that:
*   Enforces `Accept: application/x-gbp`.
*   Maintains persistent `GbpDecoder` state to share **dictionary contexts** across requests (Advanced feature: "Dictionary Training").
*   Uses `reqwest` with a custom connection pool.

### 4.3. `GbpMerger`
A dedicated merger that takes multiple `GbpValue` (binary-optimized value structs) and stitches them together.

*   **Standard Merge**: `JSON + JSON -> JSON` (Slow)
*   **GBP Merge**: `GBP + GBP -> GBP` (Fast)
    *   Deduplication tables are merged.
    *   References are remapped.
    *   No string allocations occur until the final flush.

## 5. Protocol Flow

### Step 1: Query Planning
*   Incoming Query: `query { user(id: 1) { name reviews { body } } }`
*   Plan:
    1.  Fetch `User` from **UserSubgraph**.
    2.  Fetch `Reviews` from **ReviewSubgraph** using `User.id`.

### Step 2: Parallel Fetch (The "Scatter")
*   **Request A (User)**: `POST /graphql` -> `Accept: application/x-gbp`
*   **Request B (Reviews)**: `POST /graphql` -> `Accept: application/x-gbp`

### Step 3: GBP Decoding (The "Gather")
*   **UserSubgraph** returns **400 bytes** (vs 5KB JSON).
    *   Router decodes in **20µs**.
*   **ReviewSubgraph** returns **120 bytes** (vs 2KB JSON).
    *   Router decodes in **10µs**.

### Step 4: Resolution & Response
*   Router stitches the `User` and `Review` trees.
*   If downstream client requested GBP: **Zero-Allocation Passthrough**.
    *   The internal structures are simply re-encoded (or blindly concatenated if optimized) and sent.

## 6. Implementation Strategy

### Phase 1: The "Simple" Router (Current Capability)
Use `grpc_graphql_gateway` as a subgraph host. Use the standard `async-graphql` federation support, but wired up with the newly upgraded `RestConnector` that supports GBP.

*   **Status**: **POSSIBLE NOW**.
*   **Action**: Configure `RestConnector` to auto-detect `application/x-gbp` (Done).

### Phase 2: The Independent Router
Build a dedicated binary `gbp-router` that only performs routing, stripping out the gRPC backend logic to minimize footprint.

*   **Features**:
    *   Hot-reloading Supergraph Schema.
    *   Prometheus Metrics for "Compression Ratio" and "GBP Hit Rate".
    *   Distributed Tracing (OpenTelemetry) preserved across binary boundaries.

## 7. Performance Targets

| Metric | Apollo Router (Rust) | GBP Router (Rust) | Improvement |
|:-------|:---------------------|:------------------|:------------|
| **P99 Latency** | 5ms | **< 1ms** | **5x Faster** |
| **Throughput (10 subgraphs)** | 25k RPS | **120k RPS** | **4.8x Higher** |
| **Bandwidth (Internal)** | 1 Gbps | **10 Mbps** | **99% Savings** |

## 8. Conclusion

By controlling both ends of the wire (Router and Subgraph), `grpc_graphql_gateway` can completely bypass the text-based inefficiencies of the web. The **GBP Federation Router** transforms a mesh of microservices into what functionally behaves like a **single monad**, sharing memory structures over the network with near-zero overhead.
