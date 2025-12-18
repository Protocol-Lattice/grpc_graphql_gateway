import * as lz4 from 'lz4js';
import * as pako from 'pako';

/**
 * GbpDecoder handles decompression and decoding of GraphQL Binary Protocol (GBP) payloads.
 * GBP is a structural binary format optimized for high compression ratios (up to 99%)
 * on redundant GraphQL responses.
 */
export class GbpDecoder {
    private stringPool: string[] = [];
    private shapePool: number[][] = [];
    private valuePool: any[] = [];
    private cursor: number = 0;
    private buffer: Uint8Array = new Uint8Array(0);

    constructor() { }

    /**
     * Decodes an LZ4-compressed GBP payload.
     * @param compressed The compressed GBP binary data.
     * @returns The decoded GraphQL response object.
     */
    public decodeLz4(compressed: Uint8Array): any {
        const decompressed = lz4.decompress(compressed);
        return this.decode(decompressed);
    }

    /**
     * Decodes a Gzip-compressed GBP payload.
     * @param compressed The compressed GBP binary data.
     * @returns The decoded GraphQL response object.
     */
    public decodeGzip(compressed: Uint8Array): any {
        const decompressed = pako.ungzip(compressed);
        return this.decode(decompressed);
    }

    /**
     * Decodes a raw GBP payload.
     * @param data The raw GBP binary data.
     * @returns The decoded GraphQL response object.
     */
    public decode(data: Uint8Array): any {
        this.buffer = data;
        this.cursor = 0;
        this.stringPool = [];
        this.shapePool = [];
        this.valuePool = [];

        // Find magic bytes: GBP\x08 (flexible offset to handle transport framing)
        let found = false;
        for (let i = 0; i < data.length - 4; i++) {
            if (data[i] === 0x47 && data[i + 1] === 0x42 && data[i + 2] === 0x50 && data[i + 3] === 0x08) {
                this.cursor = i + 4;
                found = true;
                break;
            }
        }

        if (!found) {
            // Minimal recovery logic for malformed packets that might omit the MAGIC prefix
            if (data[0] === 0x08 && data[1] > 0) {
                this.cursor = 1;
            } else {
                throw new Error('Invalid magic bytes: GBP\x08 not found');
            }
        }

        // 1. Decode String Pool
        const stringPoolLen = this.readVarint();
        for (let i = 0; i < stringPoolLen; i++) {
            const len = this.readVarint();
            const bytes = this.readBytes(len);
            this.stringPool.push(new TextDecoder().decode(bytes));
        }

        // 2. Decode Shape Pool
        const shapePoolLen = this.readVarint();
        for (let i = 0; i < shapePoolLen; i++) {
            const len = this.readVarint();
            const shape: number[] = [];
            for (let j = 0; j < len; j++) {
                shape.push(this.readVarint());
            }
            this.shapePool.push(shape);
        }

        // 3. Recursively decode data structure
        return this.decodeRecursive();
    }

    private decodeRecursive(): any {
        const tag = this.buffer[this.cursor++];

        if (tag === 0x08) { // Value Pool Reference (Deduplication)
            const idx = this.readVarint();
            return this.valuePool[idx];
        }

        let value: any;
        switch (tag) {
            case 0x00: // Null
                value = null;
                break;
            case 0x01: // True
                value = true;
                break;
            case 0x02: // False
                value = false;
                break;
            case 0x03: // Int64 (ZigZag Varint)
                value = this.readVarintI64();
                break;
            case 0x04: // Float64 (Little Endian)
                const view = new DataView(this.buffer.buffer, this.buffer.byteOffset + this.cursor, 8);
                value = view.getFloat64(0, true);
                this.cursor += 8;
                break;
            case 0x05: // String Pool Reference
                const sIdx = this.readVarint();
                value = this.stringPool[sIdx];
                break;
            case 0x06: // Array
                const arrLen = this.readVarint();
                value = [];
                for (let i = 0; i < arrLen; i++) {
                    value.push(this.decodeRecursive());
                }
                break;
            case 0x07: // Object
                const shapeId = this.readVarint();
                const shape = this.shapePool[shapeId];
                value = {};
                for (const keyIdx of shape) {
                    const key = this.stringPool[keyIdx];
                    value[key] = this.decodeRecursive();
                }
                break;
            case 0x09: // Columnar Array (High-performance array of objects)
                const colLen = this.readVarint();
                const colShapeId = this.readVarint();
                const colShape = this.shapePool[colShapeId];
                value = Array.from({ length: colLen }, () => ({}));
                for (const keyIdx of colShape) {
                    const key = this.stringPool[keyIdx];
                    for (let i = 0; i < colLen; i++) {
                        value[i][key] = this.decodeRecursive();
                    }
                }
                break;
            case 0x0A: // Raw String (Inline, not pooled)
                const sLen = this.readVarint();
                const sBytes = this.readBytes(sLen);
                value = new TextDecoder().decode(sBytes);
                break;
            default:
                throw new Error(`Unknown tag: 0x${tag.toString(16).padStart(2, '0')} at position ${this.cursor - 1}`);
        }

        // Add complex values to pool to support structural deduplication
        if ((typeof value === 'object' && value !== null)) {
            const isObject = !Array.isArray(value);
            const size = isObject ? Object.keys(value).length : value.length;
            if (size > 1) {
                this.valuePool.push(value);
            }
        }

        return value;
    }

    private readVarint(): number {
        let res = 0;
        let shift = 0;
        for (let i = 0; i < 5; i++) {
            const b = this.buffer[this.cursor++];
            res |= (b & 0x7F) << shift;
            if ((b & 0x80) === 0) return res;
            shift += 7;
        }
        throw new Error('Varint too long');
    }

    private readVarintI64(): bigint {
        let val = BigInt(0);
        let shift = BigInt(0);
        for (let i = 0; i < 10; i++) {
            const b = BigInt(this.buffer[this.cursor++]);
            val |= (b & BigInt(0x7F)) << shift;
            if ((b & BigInt(0x80)) === BigInt(0)) {
                // ZigZag decode: (n >> 1) ^ -(n & 1)
                return (val >> BigInt(1)) ^ -(val & BigInt(1));
            }
            shift += BigInt(7);
        }
        throw new Error('Varint too long for i64');
    }

    private readBytes(len: number): Uint8Array {
        const bytes = this.buffer.slice(this.cursor, this.cursor + len);
        this.cursor += len;
        return bytes;
    }
}
