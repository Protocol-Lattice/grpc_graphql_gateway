#!/usr/bin/env node

/**
 * Test script for Apollo Router Live Queries
 * 
 * Tests the @live directive with WebSocket connections to the GBP Router
 */

const WebSocket = require('ws');

const ROUTER_URL = 'ws://localhost:4000/graphql/live';

async function testLiveQuery() {
    console.log('🚀 Testing Apollo Router Live Queries');
    console.log('📡 Connecting to:', ROUTER_URL);

    const ws = new WebSocket(ROUTER_URL, 'graphql-transport-ws');

    ws.on('open', () => {
        console.log('✅ WebSocket connection established');

        // Step 1: Initialize connection
        console.log('\n📤 Sending connection_init');
        ws.send(JSON.stringify({
            type: 'connection_init',
            payload: {}
        }));
    });

    ws.on('message', (data) => {
        const message = JSON.parse(data.toString());
        console.log('\n📥 Received:', message.type);

        if (message.type === 'connection_ack') {
            console.log('✅ Connection acknowledged');
            console.log('   Payload:', JSON.stringify(message.payload, null, 2));

            // Step 2: Subscribe to a live query
            console.log('\n📤 Sending @live query subscription');
            ws.send(JSON.stringify({
                id: 'live-1',
                type: 'subscribe',
                payload: {
                    query: `
                        query @live {
                            users {
                                id
                                name
                                email
                            }
                        }
                    `
                }
            }));
        } else if (message.type === 'next') {
            console.log('📊 Data update received:');
            console.log('   Subscription ID:', message.id);
            console.log('   Revision:', message.payload.revision);
            console.log('   Timestamp:', message.payload.timestamp);
            console.log('   Data:', JSON.stringify(message.payload.data, null, 2));

            // Simulate: after receiving initial data, trigger an invalidation
            // In real usage, mutations in subgraphs would trigger invalidation
            if (message.payload.revision === 0) {
                console.log('\n💡 Tip: Trigger invalidation from subgraph mutations');
                console.log('   Example: LIVE_QUERY_STORE.invalidate(InvalidationEvent::new("User", "update"))');
            }
        } else if (message.type === 'complete') {
            console.log('✅ Subscription completed');
        } else if (message.type === 'error') {
            console.error('❌ Error:', message.payload);
        }
    });

    ws.on('error', (error) => {
        console.error('❌ WebSocket error:', error.message);
    });

    ws.on('close', () => {
        console.log('\n🔌 WebSocket connection closed');
    });

    // Keep the connection alive for 30 seconds
    setTimeout(() => {
        console.log('\n⏱️  Test duration complete, closing connection');
        ws.send(JSON.stringify({
            id: 'live-1',
            type: 'complete'
        }));
        ws.close();
    }, 30000);
}

// Run the test
testLiveQuery().catch(console.error);
