#!/usr/bin/env node
// Test WebSocket live query with mutation
const WebSocket = require('ws');
const http = require('http');

console.log('=== Live Query WebSocket Test with Mutation ===\n');

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

ws.on('open', () => {
    console.log('   ✓ Connected to WebSocket\n');

    // Step 1: Send connection_init
    console.log('2. Sending connection_init...');
    ws.send(JSON.stringify({ type: 'connection_init' }));
});

ws.on('message', async (data) => {
    const msg = JSON.parse(data.toString());

    if (msg.type === 'connection_ack') {
        console.log('   ✓ Connection acknowledged\n');

        // Step 2: Subscribe with @live query
        console.log('3. Subscribing to @live query: users { users { id name } }');
        ws.send(JSON.stringify({
            id: 'live-1',
            type: 'subscribe',
            payload: {
                query: 'query @live { users { users { id name } total_count } }'
            }
        }));
    }

    if (msg.type === 'next') {
        const users = msg.payload?.data?.users;
        if (users) {
            console.log(`   ✓ Received data: ${users.total_count} users`);
            users.users.forEach(u => console.log(`     - ${u.id}: ${u.name}`));
        } else if (msg.payload?.errors) {
            console.log('   ✗ Error:', msg.payload.errors[0].message);
        }
        console.log('');
    }

    if (msg.type === 'complete' && msg.id === 'live-1') {
        console.log('   ✓ Initial query completed\n');

        // Step 3: Send a mutation via HTTP
        console.log('4. Sending mutation: deleteUser(id: "2")');
        try {
            const result = await sendMutation('mutation { deleteUser(id: "2") { success message } }');
            console.log(`   ✓ Mutation result: ${result.data?.deleteUser?.message || JSON.stringify(result)}\n`);
        } catch (e) {
            console.log(`   ✗ Mutation failed: ${e.message}\n`);
        }

        // Step 4: Subscribe again to see the updated data
        console.log('5. Re-subscribing to see updated data...');
        ws.send(JSON.stringify({
            id: 'live-2',
            type: 'subscribe',
            payload: {
                query: 'query @live { users { users { id name } total_count } }'
            }
        }));
    }

    if (msg.type === 'complete' && msg.id === 'live-2') {
        console.log('   ✓ Second query completed\n');

        // Step 5: Create a new user
        console.log('6. Sending mutation: createUser(name: "David", email: "david@example.com")');
        try {
            const result = await sendMutation('mutation { createUser(name: "David", email: "david@example.com") { id name } }');
            console.log(`   ✓ Created user: ${result.data?.createUser?.name} (id: ${result.data?.createUser?.id})\n`);
        } catch (e) {
            console.log(`   ✗ Mutation failed: ${e.message}\n`);
        }

        // Step 6: Query one more time
        console.log('7. Final query to see all changes...');
        ws.send(JSON.stringify({
            id: 'live-3',
            type: 'subscribe',
            payload: {
                query: 'query @live { users { users { id name } total_count } }'
            }
        }));
    }

    if (msg.type === 'complete' && msg.id === 'live-3') {
        console.log('   ✓ Test completed!\n');
        console.log('=== Summary ===');
        console.log('The @live directive allows queries to be executed over WebSocket.');
        console.log('Mutations trigger invalidation events that can notify live queries.');
        console.log('');
        ws.close();
    }

    if (msg.type === 'error') {
        console.log('   ✗ Error:', JSON.stringify(msg.payload, null, 2));
    }
});

ws.on('error', (err) => {
    console.error('✗ WebSocket error:', err.message);
});

ws.on('close', () => {
    console.log('Connection closed');
    process.exit(0);
});

// Timeout after 15 seconds
setTimeout(() => {
    console.log('✗ Timeout - closing');
    ws.close();
    process.exit(1);
}, 15000);
