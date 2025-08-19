#!/usr/bin/env node

const WebSocket = require('ws');

// Test WebSocket connection to the deployed MLS Gateway
const RELAY_URL = 'wss://loxation-messaging-4dygmq5xta-uc.a.run.app';

console.log(`Connecting to MLS Gateway at ${RELAY_URL}...`);

const ws = new WebSocket(RELAY_URL);

ws.on('open', () => {
    console.log('✅ WebSocket connection established');
    
    // Send a REQ message to test basic Nostr functionality
    const reqMessage = JSON.stringify([
        "REQ",
        "test-subscription-id",
        {
            "kinds": [445, 446],  // MLS kinds
            "limit": 10
        }
    ]);
    
    console.log('📤 Sending REQ message:', reqMessage);
    ws.send(reqMessage);
    
    // Close after 5 seconds
    setTimeout(() => {
        ws.close();
    }, 5000);
});

ws.on('message', (data) => {
    try {
        const message = JSON.parse(data.toString());
        console.log('📥 Received:', message);
    } catch (error) {
        console.log('📥 Received (raw):', data.toString());
    }
});

ws.on('close', (code, reason) => {
    console.log(`🔌 Connection closed: ${code} ${reason}`);
    process.exit(0);
});

ws.on('error', (error) => {
    console.error('❌ WebSocket error:', error.message);
    process.exit(1);
});