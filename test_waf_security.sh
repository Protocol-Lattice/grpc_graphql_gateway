#!/bin/bash

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${YELLOW}🚀 Starting WAF Security Test Environment...${NC}"

# Cleanup function
cleanup() {
    echo -e "${YELLOW}🧹 Cleaning up processes...${NC}"
    kill $ROUTER_PID $USERS_PID $PRODUCTS_PID $REVIEWS_PID 2>/dev/null
    wait $ROUTER_PID $USERS_PID $PRODUCTS_PID $REVIEWS_PID 2>/dev/null
    echo -e "${GREEN}✅ Environment stopped.${NC}"
}
trap cleanup EXIT

# Kill existing ports
lsof -ti:4000,4002,4003,4004 | xargs kill -9 2>/dev/null

# 1. Start Subgraphs
echo "Starting Users Subgraph (4002)..."
cargo run --bin subgraph-users > users.log 2>&1 &
USERS_PID=$!

echo "Starting Products Subgraph (4003)..."
cargo run --bin subgraph-products > products.log 2>&1 &
PRODUCTS_PID=$!

echo "Starting Reviews Subgraph (4004)..."
cargo run --bin subgraph-reviews > reviews.log 2>&1 &
REVIEWS_PID=$!

# Wait for subgraphs
sleep 5

# 2. Start Router
echo "Starting Router (4000)..."
# Ensure we use the examples/router.yaml which points to ports 4002-4004
cargo run --bin router -- examples/router.yaml > router.log 2>&1 &
ROUTER_PID=$!

# Wait for router
echo "Waiting for router to be ready..."
for i in {1..30}; do
    if curl -s http://127.0.0.1:4000/health | grep -q "healthy"; then
        echo -e "${GREEN}✅ Router is ready!${NC}"
        break
    fi
    if [ $i -eq 30 ]; then
        echo -e "${RED}❌ Router failed to start${NC}"
        cat router.log
        exit 1
    fi
    sleep 1
done

# 3. Running WAF Tests
echo -e "\n${YELLOW}🛡️  Running WAF Security Tests...${NC}\n"

ROUTER_URL="http://127.0.0.1:4000/graphql"

test_attack() {
    local name=$1
    local payload=$2
    local expected_msg=$3

    echo -n "Testing $name... "
    response=$(curl -v -s -X POST "$ROUTER_URL" \
        -H 'Content-Type: application/json' \
        -d "$payload" 2>&1)
    
    # Check if curl failed (empty response usually means connection reset or similar)
    if [ -z "$response" ]; then
         echo -e "${RED}[FAILED]${NC} - No response from server (Crash?)"
         echo "Router Log (tail 20):"
         tail -n 20 router.log
         return 1
    fi

    if echo "$response" | grep -q "$expected_msg"; then
         echo -e "${GREEN}[BLOCKED]${NC} - Detected and blocked."
    elif echo "$response" | grep -q "errors"; then
         # Check if it was a WAF error or generic error
         if echo "$response" | grep -q "Potential"; then
             echo -e "${GREEN}[BLOCKED]${NC} - Blocked with generic message."
         else
             echo -e "${YELLOW}[WARNING]${NC} - Request failed but maybe not by WAF: $response"
         fi
    else
         echo -e "${RED}[FAILED]${NC} - Attack was NOT blocked!"
         echo "Response: $response"
    fi
}

# SQL Injection Test
# "1 OR 1=1" in a variable. 
# We need a query that takes a string variable. "user(id: ID!)" is a good candidate.
SQLI_PAYLOAD='{"query":"query($id:ID!){user(id:$id){name}}","variables":{"id":"u1 OR 1=1"}}'
test_attack "SQL Injection (variable)" "$SQLI_PAYLOAD" "SQL Injection"

# XSS Test
# <script> in variable
XSS_PAYLOAD='{"query":"query($id:ID!){user(id:$id){name}}","variables":{"id":"<script>alert(1)</script>"}}'
test_attack "XSS (variable)" "$XSS_PAYLOAD" "XSS"

# NoSQL Injection Test
# MongoDB $ne operator
NOSQL_PAYLOAD='{"query":"query($id:ID!){user(id:$id){name}}","variables":{"id":{"$ne": "u1"}}}'
test_attack "NoSQL Injection (variable)" "$NOSQL_PAYLOAD" "NoSQL Injection"

# SQL Comment Injection
COMMENT_PAYLOAD='{"query":"query($id:ID!){user(id:$id){name}}","variables":{"id":"u1; DROP TABLE users; --"}}'
test_attack "SQL Injection (comment)" "$COMMENT_PAYLOAD" "SQL Injection"

# Tautology SQLi
TAUTOLOGY_PAYLOAD='{"query":"query($id:ID!){user(id:$id){name}}","variables":{"id":"u1\" or true --"}}'
test_attack "SQL Injection (tautology)" "$TAUTOLOGY_PAYLOAD" "SQL Injection"

# Advanced XSS (Event Handler)
XSS_EVENT_PAYLOAD='{"query":"query($id:ID!){user(id:$id){name}}","variables":{"id":"\" onmouseover=alert(1) \""}}'
test_attack "XSS (event handler)" "$XSS_EVENT_PAYLOAD" "XSS"

# Command Injection
CMDI_PAYLOAD='{"query":"query($id:ID!){user(id:$id){name}}","variables":{"id":"; cat /etc/passwd"}}'
test_attack "Command Injection (simple)" "$CMDI_PAYLOAD" "Command Injection"

# Path Traversal
TRAVERSAL_PAYLOAD='{"query":"query($id:ID!){user(id:$id){name}}","variables":{"id":"../../../../etc/passwd"}}'
test_attack "Path Traversal (etc/passwd)" "$TRAVERSAL_PAYLOAD" "Path Traversal"

# LDAP Injection
LDAP_PAYLOAD='{"query":"query($id:ID!){user(id:$id){name}}","variables":{"id":"admin=*"}}'
test_attack "LDAP Injection (*)" "$LDAP_PAYLOAD" "LDAP Injection"

# SSTI
SSTI_PAYLOAD='{"query":"query($id:ID!){user(id:$id){name}}","variables":{"id":"{{7*7}}"}}'
test_attack "SSTI (Mustache)" "$SSTI_PAYLOAD" "SSTI"

echo -e "\n${GREEN}✅ WAF Verification Completed.${NC}"
exit 0
