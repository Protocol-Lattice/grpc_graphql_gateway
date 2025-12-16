#!/bin/bash
# Test script for Federation GraphQL endpoints

echo "🧪 Testing Federation GraphQL Endpoints"
echo "========================================"
echo ""

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Test function
test_query() {
    local name=$1
    local url=$2
    local query=$3
    
    echo -e "${BLUE}Testing: ${name}${NC}"
    echo "Endpoint: $url"
    echo "Query: $query"
    echo ""
    
    response=$(curl -s -X POST "$url" \
        -H 'Content-Type: application/json' \
        -d "{\"query\": \"$query\"}")
    
    echo -e "${GREEN}Response:${NC}"
    echo "$response" | jq . 2>/dev/null || echo "$response"
    echo ""
    echo "---"
    echo ""
}

# User Subgraph Tests
echo -e "${YELLOW}=== User Subgraph (Port 8891) ===${NC}"
echo ""

test_query "Schema Type" \
    "http://127.0.0.1:8891/graphql" \
    "{ __typename }"

test_query "Get User" \
    "http://127.0.0.1:8891/graphql" \
    "query { user(id: \\\"u1\\\") { id email name } }"

test_query "List All Users" \
    "http://127.0.0.1:8891/graphql" \
    "query { users { id name email } }"

# Product Subgraph Tests
echo -e "${YELLOW}=== Product Subgraph (Port 8892) ===${NC}"
echo ""

test_query "Get Product" \
    "http://127.0.0.1:8892/graphql" \
    "query { product(upc: \\\"apollo-1\\\") { upc name price } }"

test_query "Product with Creator" \
    "http://127.0.0.1:8892/graphql" \
    "query { product(upc: \\\"apollo-1\\\") { upc name price createdBy { id name } } }"

# Review Subgraph Tests
echo -e "${YELLOW}=== Review Subgraph (Port 8893) ===${NC}"
echo ""

test_query "User Reviews" \
    "http://127.0.0.1:8893/graphql" \
    "query { userReviews(userId: \\\"u1\\\") { id rating body } }"

test_query "Review with Relations" \
    "http://127.0.0.1:8893/graphql" \
    "query { userReviews(userId: \\\"u1\\\") { id rating body author { id name } product { upc name } } }"

# Entity Resolution Tests
echo -e "${YELLOW}=== Entity Resolution Tests ===${NC}"
echo ""

test_query "Resolve User Entity" \
    "http://127.0.0.1:8891/graphql" \
    "query { _entities(representations: [{ __typename: \\\"federation_example_User\\\", id: \\\"u1\\\" }]) { ... on federation_example_User { id email name } } }"

# Introspection
echo -e "${YELLOW}=== Introspection ===${NC}"
echo ""

test_query "Schema Introspection" \
    "http://127.0.0.1:8891/graphql" \
    "{ __schema { queryType { name } } }"

# Health Check (if available)
echo -e "${YELLOW}=== Health Checks ===${NC}"
echo ""

for port in 8891 8892 8893; do
    echo -e "${BLUE}Health check: Port $port${NC}"
    health_response=$(curl -s -f "http://127.0.0.1:$port/health" 2>&1 || echo "Not available")
    echo "Response: $health_response"
    echo ""
done

# Metrics Check
echo -e "${YELLOW}=== Metrics ===${NC}"
echo ""

for port in 9091 9092 9093; do
    echo -e "${BLUE}Metrics: Port $port${NC}"
    metrics=$(curl -s "http://127.0.0.1:$port/metrics" 2>&1 | head -5)
    if [ $? -eq 0 ]; then
        echo "$metrics"
        echo "... (truncated)"
    else
        echo "Not available on port $port"
    fi
    echo ""
done

echo -e "${GREEN}✅ All tests completed!${NC}"
echo ""
echo "💡 Tips:"
echo "  - Use browser GraphQL Playground at http://localhost:8891/graphql"
echo "  - Check metrics at http://localhost:9090/metrics"
echo "  - Federation guide: docs/src/federation/overview.md"
