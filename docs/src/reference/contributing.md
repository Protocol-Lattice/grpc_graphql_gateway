# Contributing

We welcome contributions to gRPC-GraphQL Gateway!

## Getting Started

1. Fork the repository
2. Clone your fork
3. Create a feature branch
4. Make your changes
5. Submit a pull request

## Development Setup

```bash
# Clone the repository
git clone https://github.com/Protocol-Lattice/grpc_graphql_gateway.git
cd grpc_graphql_gateway

# Build the project
cargo build

# Run tests
cargo test

# Run clippy
cargo clippy --all-targets

# Format code
cargo fmt
```

## Running Examples

```bash
# Start the greeter example
cargo run --example greeter

# Start the federation example
cargo run --example federation
```

## Project Structure

```
src/
├── lib.rs              # Re-exports and module definitions
├── gateway.rs          # Main Gateway and GatewayBuilder
├── schema.rs           # GraphQL schema generation
├── grpc_client.rs      # gRPC client management
├── federation.rs       # Apollo Federation support
├── middleware.rs       # Middleware trait and types
├── cache.rs            # Response caching
├── compression.rs      # Response compression
├── circuit_breaker.rs  # Circuit breaker pattern
├── persisted_queries.rs # APQ support
├── health.rs           # Health check endpoints
├── metrics.rs          # Prometheus metrics
├── tracing_otel.rs     # OpenTelemetry tracing
├── shutdown.rs         # Graceful shutdown
├── headers.rs          # Header propagation
└── ...
```

## Pull Request Guidelines

- Follow Rust naming conventions
- Add tests for new functionality
- Update documentation as needed
- Run `cargo fmt` before committing
- Ensure `cargo clippy` passes
- Update CHANGELOG.md for notable changes

## Reporting Issues

Please include:
- Rust version (`rustc --version`)
- Gateway version
- Minimal reproduction case
- Expected vs actual behavior

## License

By contributing, you agree that your contributions will be licensed under the MIT License.

## Resources

- [GitHub Repository](https://github.com/Protocol-Lattice/grpc_graphql_gateway)
- [crates.io](https://crates.io/crates/grpc_graphql_gateway)
- [docs.rs](https://docs.rs/grpc_graphql_gateway)
