#!/usr/bin/env node
// Test WebSocket live query with AUTOMATIC push updates
const WebSocket = require('ws');
const http = require('http');

console.log('=== Live Query AUTO-PUSH Test ===\n');
console.log('This test demonstrates that the server automatically pushes');
console.log('updates to live queries when mutations occur.\n');

// Helper to make HTTP mutation
function sendMutation(mutation) {
    return new Promise((resolve, reject) => {
        const data = JSON.stringify({ query: mutation });
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

console.log('1. Connecting to ws://localhost:9000/graphql/live ...');

const ws = new WebSocket('ws://localhost:9000/graphql/live', 'graphql-transport-ws');
let updateCount = 0;
let subscriptionId = 'live-users';

ws.on('open', () => {
    console.log('   ✓ Connected to WebSocket\n');

    console.log('2. Sending connection_init...');
    ws.send(JSON.stringify({ type: 'connection_init' }));
});

ws.on('message', async (data) => {
    const msg = JSON.parse(data.toString());

    if (msg.type === 'connection_ack') {
        console.log('   ✓ Connection acknowledged\n');

        console.log('3. Subscribing to LIVE query (connection stays open for updates)');
        ws.send(JSON.stringify({
            id: subscriptionId,
            type: 'subscribe',
            payload: {
                query: 'query @live { users { users { id name } total_count } }'
            }
        }));
    }

    if (msg.type === 'next' && msg.id === subscriptionId) {
        updateCount++;
        const users = msg.payload?.data?.users;

        if (users) {
            console.log(`\n   📡 UPDATE #${updateCount}: Received ${users.total_count} users`);
            users.users.forEach(u => console.log(`      - ${u.id}: ${u.name}`));

            // After first update, trigger mutations to see auto-push
            if (updateCount === 1) {
                console.log('\n4. Connection stays open! Now triggering mutations...\n');

                // Wait a bit then send mutation
                setTimeout(async () => {
                    console.log('   → Sending mutation: createUser(name: "Eve")');
                    try {
                        const result = await sendMutation('mutation { createUser(name: "Eve", email: "eve@example.com") { id name } }');
                        console.log(`   ✓ Created: ${result.data?.createUser?.name} (id: ${result.data?.createUser?.id})`);
                        console.log('   ⏳ Waiting for auto-push update...');
                    } catch (e) {
                        console.log(`   ✗ Mutation failed: ${e.message}`);
                    }
                }, 500);

                // Send another mutation after first one
                setTimeout(async () => {
                    console.log('\n   → Sending mutation: deleteUser(id: "1")');
                    try {
                        const result = await sendMutation('mutation { deleteUser(id: "1") { success message } }');
                        console.log(`   ✓ ${result.data?.deleteUser?.message}`);
                        console.log('   ⏳ Waiting for auto-push update...');
                    } catch (e) {
                        console.log(`   ✗ Mutation failed: ${e.message}`);
                    }
                }, 2000);

                // Close after some time
                setTimeout(() => {
                    console.log('\n5. Test complete! Summary:');
                    console.log(`   - Total updates received: ${updateCount}`);
                    console.log(`   - Expected: 3 (initial + 2 mutations)`);
                    console.log(`   - Auto-push working: ${updateCount >= 2 ? '✓ YES' : '✗ Check logs'}`);
                    console.log('\n   → Closing connection...');
                    ws.send(JSON.stringify({ type: 'complete', id: subscriptionId }));
                    ws.close();
                }, 5000);
            }
        } else if (msg.payload?.errors) {
            console.log('   ✗ Error:', msg.payload.errors[0].message);
        }
    }

    if (msg.type === 'error') {
        console.log('   ✗ Error:', JSON.stringify(msg.payload, null, 2));
    }
});

ws.on('error', (err) => {
    console.error('✗ WebSocket error:', err.message);
});

ws.on('close', () => {
    console.log('   ✓ Connection closed\n');
    console.log('=== Test Complete ===');
    process.exit(0);
});

// Timeout after 20 seconds
setTimeout(() => {
    console.log('\n✗ Timeout - closing');
    console.log(`   Total updates received: ${updateCount}`);
    ws.close();
    process.exit(1);
}, 20000);
