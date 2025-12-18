
import axios from 'axios';
import * as zlib from 'zlib';
import { GbpDecoder } from './GbpDecoder';

async function runTest() {
    console.log('--- Frontend Compression Test ---');
    console.log('Sending request to http://localhost:4000/graphql with Accept: application/x-gbp...');

    const query = {
        query: '{ me { id username email role } }'
    };

    try {
        const response = await axios.post('http://localhost:4000/graphql', query, {
            headers: {
                'Content-Type': 'application/json',
                'Accept': 'application/x-gbp'
            },
            responseType: 'arraybuffer'
        });

        console.log(`Status: ${response.status}`);
        console.log(`Content-Type: ${response.headers['content-type']}`);

        const responseBuffer = response.data;
        console.log(`Received payload size: ${responseBuffer.byteLength} bytes`);

        if (response.headers['content-type'] === 'application/x-gbp') {
            console.log('Decoding GBP (Gzip) payload...');
            const compressed = Buffer.from(responseBuffer);
            console.log('Compressed data (first 32 bytes):');
            console.log(Array.from(compressed.slice(0, 32)).map((b: number) => b.toString(16).padStart(2, '0')).join(' '));

            const decompressed = zlib.gunzipSync(compressed);
            console.log(`Decompressed data length: ${decompressed.length} bytes`);
            console.log('Decompressed data (first 64 bytes):');
            console.log(Array.from(decompressed.slice(0, 64)).map((b: number) => b.toString(16).padStart(2, '0')).join(' '));

            const decoder = new GbpDecoder();
            const decoded = decoder.decode(new Uint8Array(decompressed));

            console.log('\nDecoded GBP Response:');
            console.log(JSON.stringify(decoded, (_, v) => typeof v === 'bigint' ? v.toString() : v, 2));
            console.log('\n✅ TEST SUCCESSFUL: GBP (Gzip) decoded successfully!');
        } else {
            const jsonString = new TextDecoder().decode(new Uint8Array(responseBuffer));
            console.log('Response was not compressed (JSON):');
            console.log(JSON.stringify(JSON.parse(jsonString), null, 2));
        }
    } catch (error: any) {
        console.error('❌ TEST FAILED');
        if (error.response) {
            console.error('Status:', error.response.status);
            const errorData = new Uint8Array(error.response.data);
            console.error('Data:', new TextDecoder().decode(errorData));
        } else {
            console.error('Error:', error.message);
        }
    }
}

runTest();
