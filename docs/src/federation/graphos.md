# Apollo GraphOS & Schema Registry

Apollo GraphOS is a platform for building, managing, and scaling your supergraph. It provides a Schema Registry that acts as the source of truth for your supergraph's schema, enabling Managed Federation.

## Why use GraphOS?

- **Managed Federation**: GraphOS handles supergraph composition for you.
- **Schema Checks**: Validate changes against production traffic before deploying.
- **Explorer**: A powerful IDE for your supergraph.
- **Metrics**: Detailed usage statistics and performance monitoring.

## Prerequisites

1. An [Apollo Studio](https://studio.apollographql.com/) account.
2. The [Rover CLI](https://www.apollographql.com/docs/rover/getting-started) installed.
3. A created Graph in Apollo Studio (of type "Supergraph").

## Publishing Subgraphs

Instead of composing the supergraph locally, you publish each subgraph's schema to the GraphOS Registry. GraphOS then composes them into a supergraph schema.

### 1. Introspect Your Subgraph

First, start your `grpc-graphql-gateway` instance. Then, verify you can fetch the SDL:

```bash
# Example for the 'users' subgraph running on port 8891
rover subgraph introspect http://localhost:8891/graphql > users.graphql
```

### 2. Publish the Subgraph

Use `rover` to publish the schema to your graph variant (e.g., `current` or `production`).

```bash
# Replace MY_GRAPH with your Graph ID and 'users' with your subgraph name
rover subgraph publish MY_GRAPH@current \
  --name users \
  --schema ./users.graphql \
  --routing-url http://users-service:8891/graphql
```

Repeat this for all your subgraphs (e.g., `products`, `reviews`).

## Automatic Composition

Once subgraphs are published, GraphOS automatically composes the supergraph schema.

You can view the status and build errors in the **Build** tab in Apollo Studio.

## Fetching the Supergraph Schema

Your Apollo Router (or Gateway) needs the composed supergraph schema. With GraphOS, you have two options:

### Option A: Apollo Uplink (Recommended)

Configure Apollo Router to fetch the configuration directly from GraphOS. This allows for dynamic updates without restarting the router.

Set the `APOLLO_KEY` and `APOLLO_GRAPH_REF` environment variables:

```bash
export APOLLO_KEY=service:MY_GRAPH:your-api-key
export APOLLO_GRAPH_REF=MY_GRAPH@current

./router
```

### Option B: CI/CD Fetch

Fetch the supergraph schema during your build process:

```bash
rover supergraph fetch MY_GRAPH@current > supergraph.graphql

./router --supergraph supergraph.graphql
```

## Schema Checks

Before deploying a change, run a schema check to ensure it doesn't break existing clients.

```bash
rover subgraph check MY_GRAPH@current \
  --name users \
  --schema ./users.graphql
```

## GitHub Actions Example

Here is an example workflow to check and publish your schema:

```yaml
name: Schema Registry

on:
  push:
    branches: [ main ]
  pull_request:

env:
  APOLLO_KEY: ${{ secrets.APOLLO_KEY }}
  APOLLO_VCS_COMMIT: ${{ github.event.pull_request.head.sha }}

jobs:
  schema-check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Install Rover
        run: curl -sSL https://rover.apollo.dev/nix/latest | sh
      
      - name: Start Gateway (Background)
        run: cargo run --bin users-service &
      
      - name: Introspect Schema
        run: |
          sleep 10
          ~/.rover/bin/rover subgraph introspect http://localhost:8891/graphql > users.graphql

      - name: Check Schema
        if: github.event_name == 'pull_request'
        run: |
          ~/.rover/bin/rover subgraph check MY_GRAPH@current \
            --name users \
            --schema ./users.graphql

      - name: Publish Schema
        if: github.event_name == 'push'
        run: |
          ~/.rover/bin/rover subgraph publish MY_GRAPH@current \
            --name users \
            --schema ./users.graphql \
            --routing-url http://users-service/graphql
```
