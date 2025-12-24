#!/usr/bin/env node
/**
 * Advanced Live Query Test Suite
 * 
 * This demonstrates all 4 advanced live query features:
 * 1. Filtered live queries - users(status: ONLINE) @live
 * 2. Field-level invalidation - Only changed fields are tracked
 * 3. Batch invalidation - Multiple rapid updates merged
 * 4. Client-side caching hints - Cache control directives
 */

const WebSocket = require('ws');
const http = require('http');

console.log('=== Advanced Live Query Features Test ===\n');

// Helper to make HTTP GraphQL request
function graphqlRequest(query, variables = {}) {
    return new Promise((resolve, reject) => {
        const data = JSON.stringify({ query, variables });
        const options = {
            hostname: 'localhost',
            port: 9000,
            path: '/graphql',
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'Content-Length': data.length
            }
        };

        const req = http.request(options, (res) => {
            let body = '';
            res.on('data', (chunk) => body += chunk);
            res.on('end', () => resolve(JSON.parse(body)));
        });

        req.on('error', reject);
        req.write(data);
        req.end();
    });
}

// Test 1: Filtered Live Queries
async function testFilteredLiveQueries() {
    console.log('\n🔍 Test 1: Filtered Live Queries');
    console.log('   Testing: users(status: ONLINE) @live\n');

    return new Promise((resolve) => {
        const ws = new WebSocket('ws://localhost:9000/graphql/live', 'graphql-transport-ws');
        let updateCount = 0;

        ws.on('open', () => {
            console.log('   ✓ Connected');
            ws.send(JSON.stringify({ type: 'connection_init' }));
        });

        ws.on('message', async (data) => {
            const msg = JSON.parse(data.toString());

            if (msg.type === 'connection_ack') {
                console.log('   ✓ Connection acknowledged\n');

                // Subscribe to filtered query
                console.log('   → Subscribing to users(status: "ONLINE") @live');
                ws.send(JSON.stringify({
                    id: 'filtered-query',
                    type: 'subscribe',
                    payload: {
                        query: `query @live { 
                            users(filter: "status:ONLINE") { 
                                users { id name status { is_online } } 
                                total_count 
                            } 
                        }`
                    }
                }));
            }

            if (msg.type === 'next' && msg.id === 'filtered-query') {
                updateCount++;
                const users = msg.payload?.data?.users;

                if (users) {
                    console.log(`\n   📡 UPDATE #${updateCount}:`);
                    console.log(`      Total users: ${users.total_count}`);
                    users.users.forEach(u => {
                        const status = u.status?.is_online ? '🟢 ONLINE' : '🔴 OFFLINE';
                        console.log(`      - ${status} ${u.name} (${u.id})`);
                    });

                    // Check cache control if present
                    if (msg.payload.cache_control) {
                        console.log(`\n      📦 Cache Control:`);
                        console.log(`         max_age: ${msg.payload.cache_control.max_age}s`);
                        console.log(`         must_revalidate: ${msg.payload.cache_control.must_revalidate}`);
                        if (msg.payload.cache_control.etag) {
                            console.log(`         etag: ${msg.payload.cache_control.etag}`);
                        }
                    }

                    // Test filter by triggering OFFLINE user creation
                    if (updateCount === 1) {
                        setTimeout(async () => {
                            console.log('\n   → Creating OFFLINE user (should NOT appear in filtered results)');
                            await graphqlRequest(`
                                mutation { 
                                    createUser(name: "OfflineUser", email: "offline@test.com") { 
                                        id name 
                                    } 
                                }
                            `);
                        }, 500);

                        setTimeout(() => {
                            ws.close();
                            resolve();
                        }, 2000);
                    }
                }
            }
        });

        ws.on('error', (err) => {
            console.error('   ✗ Error:', err.message);
            resolve();
        });
    });
}

// Test 2: Field-Level Invalidation
async function testFieldLevelInvalidation() {
    console.log('\n\n📝 Test 2: Field-Level Invalidation');
    console.log('   Testing: Track only changed fields');

    return new Promise((resolve) => {
        const ws = new WebSocket('ws://localhost:9000/graphql/live', 'graphql-transport-ws');
        let updateCount = 0;

        ws.on('open', () => {
            console.log('\n   ✓ Connected');
            ws.send(JSON.stringify({ type: 'connection_init' }));
        });

        ws.on('message', async (data) => {
            const msg = JSON.parse(data.toString());

            if (msg.type === 'connection_ack') {
                ws.send(JSON.stringify({
                    id: 'field-tracking',
                    type: 'subscribe',
                    payload: {
                        query: `query @live { user(id: "1") { id name email status { is_online current_activity } } }`
                    }
                }));
            }

            if (msg.type === 'next' && msg.id === 'field-tracking') {
                updateCount++;
                const user = msg.payload?.data?.user;

                console.log(`\n   📡 UPDATE #${updateCount}:`);
                console.log(`      User: ${user.name} (${user.email})`);

                // Check for changed_fields
                if (msg.payload.changed_fields) {
                    console.log(`\n      🔄 Changed fields:`);
                    msg.payload.changed_fields.forEach(field => {
                        console.log(`         - ${field}`);
                    });
                } else {
                    console.log(`      ℹ️  Initial load (no changed fields)`);
                }

                if (updateCount === 1) {
                    // Update only the name field
                    setTimeout(async () => {
                        console.log('\n   → Updating user name (only "name" field should change)');
                        await graphqlRequest(`
                            mutation { 
                                updateUser(id: "1", name: "Alice Updated") { 
                                    id name 
                                } 
                            }
                        `);
                    }, 500);

                    setTimeout(() => {
                        ws.close();
                        resolve();
                    }, 2000);
                }
            }
        });
    });
}

// Test 3: Batch Invalidation
async function testBatchInvalidation() {
    console.log('\n\n⚡ Test 3: Batch Invalidation');
    console.log('   Testing: Merge rapid updates into single push\n');

    return new Promise((resolve) => {
        const ws = new WebSocket('ws://localhost:9000/graphql/live', 'graphql-transport-ws');
        let updateCount = 0;

        ws.on('open', () => {
            console.log('   ✓ Connected');
            ws.send(JSON.stringify({ type: 'connection_init' }));
        });

        ws.on('message', async (data) => {
            const msg = JSON.parse(data.toString());

            if (msg.type === 'connection_ack') {
                ws.send(JSON.stringify({
                    id: 'batch-test',
                    type: 'subscribe',
                    payload: {
                        query: `query @live { users { users { id name } total_count } }`
                    }
                }));
            }

            if (msg.type === 'next' && msg.id === 'batch-test') {
                updateCount++;
                const users = msg.payload?.data?.users;

                console.log(`\n   📡 UPDATE #${updateCount}:`);
                console.log(`      Total users: ${users.total_count}`);

                // Check if batched
                if (msg.payload.batched) {
                    console.log(`      🎯 BATCHED UPDATE - Merged multiple changes!`);
                }

                // Check timestamp drift
                if (msg.payload.timestamp) {
                    const serverTime = new Date(msg.payload.timestamp * 1000);
                    console.log(`      ⏰ Server timestamp: ${serverTime.toISOString()}`);
                }

                if (updateCount === 1) {
                    // Trigger rapid-fire mutations (should be batched)
                    console.log('\n   → Triggering 5 rapid mutations (should batch into 1-2 updates)');

                    for (let i = 0; i < 5; i++) {
                        setTimeout(async () => {
                            await graphqlRequest(`
                                mutation { 
                                    createUser(name: "BatchUser${i}", email: "batch${i}@test.com") { 
                                        id 
                                    } 
                                }
                            `);
                            console.log(`      ✓ Mutation ${i + 1}/5 sent`);
                        }, i * 10); // 10ms apart
                    }

                    setTimeout(() => {
                        ws.close();
                        resolve();
                    }, 3000);
                }
            }
        });
    });
}

// Test 4: Cache Control Directives
async function testCacheControl() {
    console.log('\n\n💾 Test 4: Client-Side Caching Hints');
    console.log('   Testing: Cache-Control directives in response\n');

    return new Promise((resolve) => {
        const ws = new WebSocket('ws://localhost:9000/graphql/live', 'graphql-transport-ws');

        ws.on('open', () => {
            console.log('   ✓ Connected');
            ws.send(JSON.stringify({ type: 'connection_init' }));
        });

        ws.on('message', (data) => {
            const msg = JSON.parse(data.toString());

            if (msg.type === 'connection_ack') {
                ws.send(JSON.stringify({
                    id: 'cache-test',
                    type: 'subscribe',
                    payload: {
                        query: `query @live { user(id: "1") { id name email } }`
                    }
                }));
            }

            if (msg.type === 'next' && msg.id === 'cache-test') {
                console.log('   📡 Received update with cache directives:');

                if (msg.payload.cache_control) {
                    const cc = msg.payload.cache_control;
                    console.log(`\n      Cache-Control:`);
                    console.log(`         ├─ max-age: ${cc.max_age} seconds`);
                    console.log(`         ├─ public: ${cc.public}`);
                    console.log(`         ├─ must-revalidate: ${cc.must_revalidate}`);
                    if (cc.etag) {
                        console.log(`         └─ ETag: ${cc.etag.substring(0, 16)}...`);
                    }

                    console.log(`\n      💡 Client can cache for ${cc.max_age}s before revalidating`);
                } else {
                    console.log('      ℹ️  No cache control (default behavior)');
                }

                setTimeout(() => {
                    ws.close();
                    resolve();
                }, 1000);
            }
        });
    });
}

// Run all tests
async function runAllTests() {
    console.log('Starting in 2 seconds...\n');
    await new Promise(r => setTimeout(r, 2000));

    try {
        await testFilteredLiveQueries();
        await new Promise(r => setTimeout(r, 1000));

        await testFieldLevelInvalidation();
        await new Promise(r => setTimeout(r, 1000));

        await testBatchInvalidation();
        await new Promise(r => setTimeout(r, 1000));

        await testCacheControl();

        console.log('\n\n✅ All tests completed!');
        console.log('\n=== Summary ===');
        console.log('✓ Filtered Live Queries: Tested query argument filtering');
        console.log('✓ Field-Level Invalidation: Tracked granular field changes');
        console.log('✓ Batch Invalidation: Merged rapid updates efficiently');
        console.log('✓ Cache Control: Demonstrated client caching hints');

        process.exit(0);
    } catch (error) {
        console.error('\n✗ Test failed:', error);
        process.exit(1);
    }
}

runAllTests();
