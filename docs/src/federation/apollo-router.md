# Running with Apollo Router

Compose your gRPC-GraphQL Gateway subgraphs with Apollo Router to create a federated supergraph.

## Prerequisites

- [Apollo Router](https://www.apollographql.com/docs/router/) installed
- Federation-enabled gateway subgraphs running

## Step 1: Start Your Subgraphs

Start each gRPC-GraphQL Gateway as a federation subgraph:

**Users Subgraph (port 8891):**
```rust
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(USERS_DESCRIPTORS)
    .enable_federation()
    .add_grpc_client("users.UserService", user_client)
    .build()?;

gateway.serve("0.0.0.0:8891").await?;
```

**Products Subgraph (port 8892):**
```rust
let gateway = Gateway::builder()
    .with_descriptor_set_bytes(PRODUCTS_DESCRIPTORS)
    .enable_federation()
    .add_grpc_client("products.ProductService", product_client)
    .build()?;

gateway.serve("0.0.0.0:8892").await?;
```

## Step 2: Create Supergraph Configuration

Create `supergraph.yaml`:

```yaml
federation_version: =2.3.2

subgraphs:
  users:
    routing_url: http://localhost:8891/graphql
    schema:
      subgraph_url: http://localhost:8891/graphql
  
  products:
    routing_url: http://localhost:8892/graphql
    schema:
      subgraph_url: http://localhost:8892/graphql
```

## Step 3: Compose the Supergraph

Install Rover CLI:

```bash
curl -sSL https://rover.apollo.dev/nix/latest | sh
```

Compose the supergraph:

```bash
rover supergraph compose --config supergraph.yaml > supergraph.graphql
```

## Step 4: Run Apollo Router

```bash
router --supergraph supergraph.graphql --dev
```

Or with configuration:

```bash
router \
  --supergraph supergraph.graphql \
  --config router.yaml
```

## Router Configuration

Create `router.yaml` for production:

```yaml
supergraph:
  listen: 0.0.0.0:4000
  introspection: true

cors:
  origins:
    - https://studio.apollographql.com

telemetry:
  exporters:
    tracing:
      otlp:
        enabled: true
        endpoint: http://jaeger:4317

health_check:
  listen: 0.0.0.0:8088
  enabled: true
  path: /health
```

## Querying the Supergraph

Once running, query through the router:

```graphql
query {
  user(id: "123") {
    id
    name
    email
    orders {      # From Orders subgraph
      id
      total
      products {  # From Products subgraph
        upc
        name
        price
      }
    }
  }
}
```

## Docker Compose Example

```yaml
version: '3.8'

services:
  router:
    image: ghcr.io/apollographql/router:v1.25.0
    ports:
      - "4000:4000"
    volumes:
      - ./supergraph.graphql:/supergraph.graphql
      - ./router.yaml:/router.yaml
    command: --supergraph /supergraph.graphql --config /router.yaml

  users-gateway:
    build: ./users-gateway
    ports:
      - "8891:8888"
    depends_on:
      - users-grpc

  products-gateway:
    build: ./products-gateway
    ports:
      - "8892:8888"
    depends_on:
      - products-grpc

  users-grpc:
    build: ./users-service
    ports:
      - "50051:50051"

  products-grpc:
    build: ./products-service
    ports:
      - "50052:50052"
```

## Continuous Composition

For production environments, we recommend using Apollo GraphOS for managed federation and continuous delivery.

See the [GraphOS & Schema Registry](./graphos.md) guide for detailed instructions on publishing subgraphs and setting up CI/CD pipelines.

## Troubleshooting

### Subgraph Schema Fetch Fails

Ensure the subgraph introspection is enabled and accessible:

```bash
curl http://localhost:8891/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "{ _service { sdl } }"}'
```

### Entity Resolution Errors

Check that:
1. Entity resolvers are configured for all entity types
2. gRPC clients are connected
3. Key fields match between subgraphs

### Composition Errors

Run composition with verbose output:

```bash
rover supergraph compose --config supergraph.yaml --log debug
```
