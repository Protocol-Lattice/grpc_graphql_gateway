#!/bin/bash
set -e

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

cleanup() {
    if [ ! -z "$SERVER_PID" ]; then
        echo "Stopping server..."
        kill $SERVER_PID 2>/dev/null || true
    fi
}
trap cleanup EXIT

echo -e "${YELLOW}Building binaries (release mode)...${NC}"
cargo build --release --bin greeter --features yaml
cargo build --release --bin benchmark

# --- SETUP ---
echo "Cleaning up ports..."
lsof -ti:50051 | xargs kill -9 2>/dev/null || true
lsof -ti:8888 | xargs kill -9 2>/dev/null || true
sleep 1

echo "Starting Gateway Server..."
# Using release binary directly
./target/release/greeter > server.log 2>&1 &
SERVER_PID=$!

echo "Waiting for server..."
STARTED=false
for i in {1..30}; do
    if grep -q "Gateway server listening" server.log; then STARTED=true; break; fi
    sleep 1
done

if [ "$STARTED" = false ]; then 
    echo -e "${RED}Server failed to start.${NC}"; 
    cat server.log; 
    exit 1; 
fi

echo -e "${GREEN}Server started successfully!${NC}"

echo "--------------------------------------------------------"
echo "Running Benchmark 1: Cached Requests (Gateway Overhead)"
echo "--------------------------------------------------------"
# Run the benchmark binary (Default = Cached)
./target/release/benchmark

echo ""
echo "--------------------------------------------------------"
echo "Running Benchmark 2: Uncached Requests (Full Integration)"
echo "--------------------------------------------------------"
# Run the benchmark binary with cache busting
./target/release/benchmark --disable-cache

echo -e "${GREEN}Benchmark Suite Complete.${NC}"
