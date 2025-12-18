/**
 * GbpDecoder handles decompression and decoding of GraphQL Binary Protocol (GBP) payloads.
 * GBP is a structural binary format optimized for high compression ratios (up to 99%)
 * on redundant GraphQL responses.
 */
export declare class GbpDecoder {
    private stringPool;
    private shapePool;
    private valuePool;
    private cursor;
    private buffer;
    constructor();
    /**
     * Decodes an LZ4-compressed GBP payload.
     * @param compressed The compressed GBP binary data.
     * @returns The decoded GraphQL response object.
     */
    decodeLz4(compressed: Uint8Array): any;
    /**
     * Decodes a Gzip-compressed GBP payload.
     * @param compressed The compressed GBP binary data.
     * @returns The decoded GraphQL response object.
     */
    decodeGzip(compressed: Uint8Array): any;
    /**
     * Decodes a raw GBP payload.
     * @param data The raw GBP binary data.
     * @returns The decoded GraphQL response object.
     */
    decode(data: Uint8Array): any;
    private decodeRecursive;
    private readVarint;
    private readVarintI64;
    private readBytes;
}
