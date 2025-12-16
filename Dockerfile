# Multi-stage Dockerfile for gRPC GraphQL Gateway
# Optimized for production deployment with minimal image size

# ============================================
# Stage 1: Builder
# ============================================
FROM rust:1.75-slim as builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Create a new empty project
WORKDIR /usr/src/app

# Copy manifests
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./

# Copy source code
COPY src ./src
COPY examples ./examples
COPY proto ./proto

# Build for release
RUN cargo build --release --bin greeter
RUN cargo build --release --bin federation

# ============================================
# Stage 2: Runtime
# ============================================
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 gateway && \
    mkdir -p /app/descriptors && \
    mkdir -p /app/config && \
    chown -R gateway:gateway /app

WORKDIR /app

# Copy binaries from builder
COPY --from=builder /usr/src/app/target/release/greeter /usr/local/bin/greeter
COPY --from=builder /usr/src/app/target/release/federation /usr/local/bin/federation

# Copy descriptor files (if available)
COPY --chown=gateway:gateway proto/*.bin /app/descriptors/ 2>/dev/null || true

# Switch to non-root user
USER gateway

# Expose ports
# 8080: HTTP/GraphQL
# 9090: Metrics
# 50051: gRPC
EXPOSE 8080 9090 50051

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

# Default command
CMD ["greeter"]
