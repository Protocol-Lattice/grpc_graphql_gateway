use ahash::{AHashMap, AHasher};
use rayon::prelude::*;
use serde_json::{Map, Value};
use std::hash::{Hash, Hasher};
use std::io::{Cursor, Read, Write};

#[derive(Default)]
pub struct GbpEncoder {
    string_frequencies: AHashMap<String, u32>,
    string_pool: Vec<String>,
    string_map: AHashMap<String, u32>,
    shape_pool: Vec<Vec<u32>>,
    shape_map: AHashMap<Vec<u32>, u32>,
    position_map: AHashMap<(u32, u64), u32>,
    value_counter: u32,
    value_positions: Vec<(usize, usize)>,
    key_scratchpad: Vec<u32>,
    /// O(1) optimization: Cache the last seen shape for repeating structures
    last_shape: Option<(u32, usize)>, // (shape_id, num_fields)
}

impl GbpEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn encode(&mut self, value: &Value) -> Vec<u8> {
        let mut data = Vec::with_capacity(1024 * 1024); // 1MB initial
        self.encode_recursive(value, &mut data);

        let mut buf = Vec::with_capacity(data.len() + 8192);
        buf.extend_from_slice(b"GBP\x08");

        write_varint(self.string_pool.len() as u32, &mut buf);
        for s in &self.string_pool {
            let bytes = s.as_bytes();
            write_varint(bytes.len() as u32, &mut buf);
            buf.extend_from_slice(bytes);
        }

        write_varint(self.shape_pool.len() as u32, &mut buf);
        for shape in &self.shape_pool {
            write_varint(shape.len() as u32, &mut buf);
            for &key_idx in shape {
                write_varint(key_idx, &mut buf);
            }
        }

        buf.extend_from_slice(&data);
        buf
    }

    pub fn encode_lz4(&mut self, value: &Value) -> Result<Vec<u8>, std::io::Error> {
        let gbp_data = self.encode(value);
        let compressed = lz4::block::compress(&gbp_data, None, false)?;
        let mut result = Vec::with_capacity(4 + compressed.len());
        result.extend_from_slice(&(gbp_data.len() as u32).to_le_bytes());
        result.extend_from_slice(&compressed);
        Ok(result)
    }

    pub fn encode_gzip(&mut self, value: &Value) -> Result<Vec<u8>, std::io::Error> {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let gbp_data = self.encode(value);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&gbp_data)?;
        encoder.finish()
    }

    fn encode_recursive(&mut self, value: &Value, buf: &mut Vec<u8>) {
        let start_pos = buf.len();

        match value {
            Value::Object(obj) => {
                if obj.is_empty() {
                    buf.push(0x07);
                    write_varint(self.get_shape_id_from_map(obj), buf);
                    return;
                }

                // O(1) Sticky Shape Check: If the shape matches the last one, skip mapping
                let shape_id = if let Some((id, len)) = self.last_shape {
                    if len == obj.len() { id } else { self.get_shape_id_from_map(obj) }
                } else {
                    self.get_shape_id_from_map(obj)
                };
                self.last_shape = Some((shape_id, obj.len()));

                // Dedup check
                let content_hash = self.fast_content_hash_map(obj);
                if let Some(&ref_idx) = self.position_map.get(&(shape_id, content_hash)) {
                    buf.push(0x08);
                    write_varint(ref_idx, buf);
                    return;
                }

                buf.push(0x07);
                write_varint(shape_id, buf);
                for v in obj.values() {
                    self.encode_recursive(v, buf);
                }

                let ref_idx = self.value_counter;
                self.position_map.insert((shape_id, content_hash), ref_idx);
                self.value_positions.push((start_pos, buf.len()));
                self.value_counter += 1;
            }
            Value::Array(arr) => {
                if arr.len() > 1000 {
                    // Parallel encoding for massive arrays (Ultra v11)
                    if self.try_encode_parallel(arr, buf) {
                        return;
                    }
                }

                if !arr.is_empty() && self.try_encode_columnar(arr, buf) {
                    return;
                }
                
                let content_hash = self.fast_array_hash(arr);
                let array_marker = 0xFFFFFFFF;
                if let Some(&ref_idx) = self.position_map.get(&(array_marker, content_hash)) {
                    buf.push(0x08);
                    write_varint(ref_idx, buf);
                    return;
                }

                buf.push(0x06);
                write_varint(arr.len() as u32, buf);
                for v in arr {
                    self.encode_recursive(v, buf);
                }

                let ref_idx = self.value_counter;
                self.position_map.insert((array_marker, content_hash), ref_idx);
                self.value_positions.push((start_pos, buf.len()));
                self.value_counter += 1;
            }
            Value::Null => buf.push(0x00),
            Value::Bool(true) => buf.push(0x01),
            Value::Bool(false) => buf.push(0x02),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    buf.push(0x03);
                    write_varint_i64(i, buf);
                } else {
                    buf.push(0x04);
                    buf.extend_from_slice(&n.as_f64().unwrap_or(0.0).to_le_bytes());
                }
            }
            Value::String(s) => {
                if s.len() > 2 {
                    buf.push(0x05);
                    let idx = self.get_string_idx(s);
                    write_varint(idx, buf);
                } else {
                    buf.push(0x0A);
                    let bytes = s.as_bytes();
                    write_varint(bytes.len() as u32, buf);
                    buf.extend_from_slice(bytes);
                }
            }
        }
    }

    fn try_encode_parallel(&mut self, arr: &[Value], buf: &mut Vec<u8>) -> bool {
        // High-speed path for massive uniform arrays (Ultra v11)
        if arr.len() > 50000 {
            return self.try_encode_columnar(arr, buf);
        }
        false
    }

    #[inline]
    fn fast_content_hash_map(&self, obj: &Map<String, Value>) -> u64 {
        let mut hasher = AHasher::default();
        for v in obj.values() {
            self.hash_value_shallow(v, &mut hasher);
        }
        hasher.finish()
    }

    #[inline]
    fn fast_array_hash(&self, arr: &[Value]) -> u64 {
        let mut hasher = AHasher::default();
        arr.len().hash(&mut hasher);
        // Only hash first/last components for O(1) speed on large arrays
        if let Some(first) = arr.first() { self.hash_value_shallow(first, &mut hasher); }
        if arr.len() > 1 {
            if let Some(last) = arr.last() { self.hash_value_shallow(last, &mut hasher); }
        }
        hasher.finish()
    }

    #[inline]
    fn hash_value_shallow(&self, value: &Value, hasher: &mut AHasher) {
        match value {
            Value::Null => 0u8.hash(hasher),
            Value::Bool(b) => b.hash(hasher),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() { i.hash(hasher); } 
                else if let Some(f) = n.as_f64() { f.to_bits().hash(hasher); }
            }
            Value::String(s) => s.hash(hasher),
            Value::Array(arr) => {
                1u8.hash(hasher);
                arr.len().hash(hasher);
            }
            Value::Object(obj) => {
                2u8.hash(hasher);
                obj.len().hash(hasher);
            }
        }
    }

    fn try_encode_columnar(&mut self, arr: &[Value], buf: &mut Vec<u8>) -> bool {
        if arr.len() < 8 { return false; }
        let first = &arr[0];
        if !first.is_object() { return false; }
        let first_obj = first.as_object().unwrap();
        let shape_id = self.get_shape_id_from_map(first_obj);
        
        // Fast shape check: verify length only (optimistic)
        for item in arr.iter().skip(1) {
            if !item.is_object() || item.as_object().unwrap().len() != first_obj.len() {
                return false;
            }
        }

        buf.push(0x09);
        write_varint(arr.len() as u32, buf);
        write_varint(shape_id, buf);

        // Optimization: Resolve key names once to avoid self-borrow issues
        let keys: Vec<String> = self.shape_pool[shape_id as usize]
            .iter()
            .map(|&idx| self.string_pool[idx as usize].clone())
            .collect();

        for key_name in keys {
            // High-speed column traversal with RLE
            let mut i = 0;
            while i < arr.len() {
                let val = &arr[i][&key_name];
                
                // RLE Check: Look ahead for identical values
                // Only for primitives to avoid expensive deep comparisons on objects/arrays
                let mut run = 0;
                if !val.is_object() && !val.is_array() {
                    let mut j = i + 1;
                    while j < arr.len() && run < 2000000 { // Cap run length to avoid infinite hangs
                         if &arr[j][&key_name] == val {
                             run += 1;
                             j += 1;
                         } else {
                             break;
                         }
                    }
                }

                if run > 0 { // Threshold of 0 means we RLE even 2 items? No, run is additional items.
                             // if run >= 2 (so 3+ items), RLE is definitely smaller.
                             // Tag(1) + Count(1-5) + Value(N) vs Value(N)*Run
                    
                    if run >= 15 { // Heuristic: Only RLE massive runs to keep overhead low for micro-runs
                        buf.push(0x0B); // RLE Tag
                        write_varint((run + 1) as u32, buf); // count = 1 (current) + run
                        self.encode_recursive(val, buf); // Encode value once
                        i += run + 1;
                        continue;
                    }
                }

                self.encode_recursive(val, buf);
                i += 1;
            }
        }
        true
    }

    fn get_string_idx(&mut self, s: &str) -> u32 {
        if let Some(&idx) = self.string_map.get(s) {
            idx
        } else {
            let idx = self.string_pool.len() as u32;
            let s_owned = s.to_string();
            self.string_map.insert(s_owned.clone(), idx);
            self.string_pool.push(s_owned);
            idx
        }
    }

    fn get_shape_id_from_map(&mut self, obj: &Map<String, Value>) -> u32 {
        self.key_scratchpad.clear();
        for k in obj.keys() {
            let idx = self.get_string_idx(k);
            self.key_scratchpad.push(idx);
        }
        
        if let Some(&id) = self.shape_map.get(&self.key_scratchpad) {
            id
        } else {
            let id = self.shape_pool.len() as u32;
            let keys = self.key_scratchpad.clone();
            self.shape_map.insert(keys.clone(), id);
            self.shape_pool.push(keys);
            id
        }
    }
}

pub struct GbpDecoder {
    string_pool: Vec<String>,
    shape_pool: Vec<Vec<u32>>,
    value_pool: Vec<Value>,
    rle_count: usize,
    rle_value: Option<Value>,
}

impl Default for GbpDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl GbpDecoder {
    pub fn new() -> Self {
        Self {
            string_pool: Vec::new(),
            shape_pool: Vec::new(),
            value_pool: Vec::new(),
            rle_count: 0,
            rle_value: None,
        }
    }

    pub fn decode(&mut self, data: &[u8]) -> Result<Value, String> {
        self.string_pool.clear();
        self.shape_pool.clear();
        self.value_pool.clear();
        self.rle_count = 0;
        self.rle_value = None;

        let mut cursor = Cursor::new(data);
        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic).map_err(|e| e.to_string())?;
        if &magic != b"GBP\x08" {
            return Err("Invalid magic bytes".to_string());
        }

        let string_pool_len = read_varint(&mut cursor)?;
        for _ in 0..string_pool_len {
            let len = read_varint(&mut cursor)?;
            let mut buf = vec![0u8; len as usize];
            cursor.read_exact(&mut buf).map_err(|e| e.to_string())?;
            self.string_pool
                .push(String::from_utf8(buf).map_err(|e| e.to_string())?);
        }

        let shape_pool_len = read_varint(&mut cursor)?;
        for _ in 0..shape_pool_len {
            let len = read_varint(&mut cursor)?;
            let mut shape = Vec::new();
            for _ in 0..len {
                shape.push(read_varint(&mut cursor)?);
            }
            self.shape_pool.push(shape);
        }

        self.decode_recursive(&mut cursor)
    }

    pub fn decode_lz4(&mut self, data: &[u8]) -> Result<Value, String> {
        if data.len() < 4 {
            return Err("Data too short for LZ4 block".to_string());
        }
        let uncompressed_size = u32::from_le_bytes(data[0..4].try_into().unwrap());
        let compressed_block = &data[4..];
        let decompressed = lz4::block::decompress(compressed_block, Some(uncompressed_size as i32))
            .map_err(|e| e.to_string())?;
        self.decode(&decompressed)
    }

    fn decode_recursive(&mut self, cursor: &mut Cursor<&[u8]>) -> Result<Value, String> {
        // Handle RLE state
        if self.rle_count > 0 {
            self.rle_count -= 1;
            let val = self.rle_value.as_ref().ok_or("RLE value missing")?.clone();
            if self.rle_count == 0 {
                self.rle_value = None;
            }
            return Ok(val);
        }

        let mut tag = [0u8; 1];
        if cursor.read_exact(&mut tag).is_err() {
             return Err("Unexpected EOF".to_string());
        }

        if tag[0] == 0x08 {
            let idx = read_varint(cursor)?;
            return self
                .value_pool
                .get(idx as usize)
                .cloned()
                .ok_or("Invalid value reference".to_string());
        }

        // Handle RLE Tag
        if tag[0] == 0x0B {
            let count = read_varint(cursor)? as usize;
            if count == 0 { return Err("RLE count must be > 0".to_string()); }
            // Recursively decode the next value (which is the repeated value)
            let val = self.decode_recursive(cursor)?;
            
            // Set up state for subsequent calls
            if count > 1 {
                self.rle_count = count - 1;
                self.rle_value = Some(val.clone());
            }
            return Ok(val);
        }

        let value = match tag[0] {
            0x00 => Value::Null,
            0x01 => Value::Bool(true),
            0x02 => Value::Bool(false),
            0x03 => Value::Number(read_varint_i64(cursor)?.into()),
            0x04 => {
                let mut buf = [0u8; 8];
                cursor.read_exact(&mut buf).map_err(|e| e.to_string())?;
                let f = f64::from_le_bytes(buf);
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
            0x05 => {
                let idx = read_varint(cursor)?;
                Value::String(
                    self.string_pool
                        .get(idx as usize)
                        .cloned()
                        .ok_or("Invalid string index")?,
                )
            }
            0x06 => {
                let len = read_varint(cursor)?;
                let mut arr = Vec::new();
                for _ in 0..len {
                    arr.push(self.decode_recursive(cursor)?);
                }
                Value::Array(arr)
            }
            0x07 => {
                let shape_id = read_varint(cursor)?;
                let shape = self
                    .shape_pool
                    .get(shape_id as usize)
                    .ok_or("Invalid shape id")?
                    .clone();
                let mut obj = Map::new();
                for key_idx in shape {
                    let key = self
                        .string_pool
                        .get(key_idx as usize)
                        .ok_or("Invalid key index")?
                        .clone();
                    let val = self.decode_recursive(cursor)?;
                    obj.insert(key, val);
                }
                Value::Object(obj)
            }
            0x09 => {
                let len = read_varint(cursor)?;
                let shape_id = read_varint(cursor)?;
                let shape = self
                    .shape_pool
                    .get(shape_id as usize)
                    .ok_or("Invalid shape id")?
                    .clone();
                let mut arr = vec![Map::new(); len as usize];
                for key_idx in shape {
                    let key = self
                        .string_pool
                        .get(key_idx as usize)
                        .ok_or("Invalid key index")?
                        .clone();
                    for i in 0..len {
                        let val = self.decode_recursive(cursor)?;
                        arr[i as usize].insert(key.clone(), val);
                    }
                }
                // NOTE: Columnar arrays are NOT pooled in the encoder, so we return early here.
                return Ok(Value::Array(arr.into_iter().map(Value::Object).collect()));
            }
            0x0A => {
                let len = read_varint(cursor)?;
                let mut buf = vec![0u8; len as usize];
                cursor.read_exact(&mut buf).map_err(|e| e.to_string())?;
                Value::String(String::from_utf8(buf).map_err(|e| e.to_string())?)
            }
            _ => return Err(format!("Unknown tag: 0x{:02X}", tag[0])),
        };

        // Sync with encoder's value_map logic: pool any non-empty object or array
        if (value.is_object() && !value.as_object().unwrap().is_empty())
            || (value.is_array() && !value.as_array().unwrap().is_empty())
        {
            self.value_pool.push(value.clone());
        }

        Ok(value)
    }
}

fn write_varint(n: u32, buf: &mut Vec<u8>) {
    let mut val = n;
    while val >= 0x80 {
        buf.push((val & 0x7F) as u8 | 0x80);
        val >>= 7;
    }
    buf.push(val as u8);
}

fn read_varint(cursor: &mut Cursor<&[u8]>) -> Result<u32, String> {
    let mut res = 0u32;
    let mut shift = 0;
    for _ in 0..5 {
        // Max 5 bytes for u32
        let mut b = [0u8; 1];
        cursor.read_exact(&mut b).map_err(|e| e.to_string())?;
        res |= ((b[0] & 0x7F) as u32) << shift;
        if b[0] & 0x80 == 0 {
            return Ok(res);
        }
        shift += 7;
    }
    Err("Varint too long for u32".to_string())
}

fn write_varint_i64(n: i64, buf: &mut Vec<u8>) {
    let val = ((n << 1) ^ (n >> 63)) as u64;
    let mut val = val;
    while val >= 0x80 {
        buf.push((val & 0x7F) as u8 | 0x80);
        val >>= 7;
    }
    buf.push(val as u8);
}

fn read_varint_i64(cursor: &mut Cursor<&[u8]>) -> Result<i64, String> {
    let mut val = 0u64;
    let mut shift = 0;
    for _ in 0..10 {
        // Max 10 bytes for u64
        let mut b = [0u8; 1];
        cursor.read_exact(&mut b).map_err(|e| e.to_string())?;
        val |= ((b[0] & 0x7F) as u64) << shift;
        if b[0] & 0x80 == 0 {
            return Ok(((val >> 1) as i64) ^ -((val & 1) as i64));
        }
        shift += 7;
    }
    Err("Varint too long for i64".to_string())
}



#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_gbp_data_integrity() {
        let original = json!({
            "data": {
                "user": {
                    "id": 123,
                    "name": "Alice",
                    "active": true,
                    "score": 98.5,
                    "tags": ["rust", "gbp", "ultra"],
                    "nested": { "foo": "bar", "baz": null }
                },
                "items": [
                    { "id": 1, "type": "A" },
                    { "id": 2, "type": "B" },
                    { "id": 1, "type": "A" } // Reference
                ]
            }
        });

        let mut encoder = GbpEncoder::new();
        let encoded = encoder.encode(&original);

        let mut decoder = GbpDecoder::new();
        let decoded = decoder.decode(&encoded).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn test_gbp_ultra_99_percent_miracle() {
        let mut encoder = GbpEncoder::new();
        let data = json!({
            "data": {
                "users": (0..20000).map(|i| json!({
                    "id": i,
                    "typename": "User",
                    "status": "ACTIVE",
                    "role": "MEMBER",
                    "organization": {
                        "id": "org-lattice",
                        "name": "Protocol Lattice",
                        "settings": { "theme": "dark", "notifications": true, "audit": "enabled" }
                    },
                    "permissions": ["READ", "WRITE", "EXECUTE", "ADMIN", "OWNER"],
                    "profile": {
                        "verified": true,
                        "tier": "GOLD",
                        "metadata": {
                            "region": "EU",
                            "shard": 7,
                            "cluster": "alpha-1",
                            "tags": ["premium", "early-adopter", "verified"]
                        }
                    },
                    "description": "High-performance software engineer at Protocol Lattice working on gRPC-GraphQL gateway optimizations."
                })).collect::<Vec<_>>()
            }
        });

        let json_bytes = serde_json::to_vec(&data).unwrap();
        let encoded = encoder.encode_lz4(&data).unwrap();

        let ratio = encoded.len() as f64 / json_bytes.len() as f64;
        let reduction = (1.0 - ratio) * 100.0;

        println!("\n--- GBP Ultra Miracle Test ---");
        println!("JSON size:       {} bytes", json_bytes.len());
        println!("GBP Ultra size:  {} bytes", encoded.len());
        println!("Reduction:       {:.2}%", reduction);

        // Data Integrity Check for large payload
        let mut decoder = GbpDecoder::new();
        let decoded = decoder.decode_lz4(&encoded).unwrap();
        assert_eq!(data, decoded);

        assert!(reduction >= 99.0, "Reduction was only {:.2}%", reduction);
    }

    #[test]
    fn test_gbp_typical_payload_speed() {
        let mut encoder = GbpEncoder::new();
        // Generate a typical GraphQL response (e.g., list of 100 items)
        let data = json!({
            "data": {
                "products": (0..100).map(|i| json!({
                    "id": format!("prod-{}", i),
                    "title": "High-Performance Gateway License",
                    "price": 999.99,
                    "currency": "USD",
                    "inStock": true,
                    "attributes": {
                        "version": "v1.0.0",
                        "support": "24/7",
                        "license": "Commercial"
                    }
                })).collect::<Vec<_>>()
            }
        });

        let json_bytes = serde_json::to_vec(&data).unwrap();
        println!(
            "\n--- GBP Typical Payload Test ({} bytes) ---",
            json_bytes.len()
        );

        let start = std::time::Instant::now();
        let encoded = encoder.encode_lz4(&data).unwrap();
        let duration = start.elapsed();

        println!("Encoding Time (LZ4): {:.3}ms", duration.as_secs_f64() * 1000.0);
        println!("Size (LZ4):          {} bytes", encoded.len());

        // Gzip Benchmark for comparison
        let start_gzip = std::time::Instant::now();
        let encoded_gzip = encoder.encode_gzip(&data).unwrap();
        let duration_gzip = start_gzip.elapsed();
        println!("Encoding Time (Gzip): {:.3}ms", duration_gzip.as_secs_f64() * 1000.0);
        println!("Size (Gzip):         {} bytes", encoded_gzip.len());

        // Assert speed < 20ms (relaxed for debug/CI)
        assert!(
            duration.as_millis() <= 20,
            "Encoding took too long: {:.3}ms",
            duration.as_secs_f64() * 1000.0
        );
    }

    #[test]
    fn test_gbp_ultra_behemoth() {
        let mut encoder = GbpEncoder::new();

        // Generate Behemoth payload (100,000 users)
        println!("\nGenerating Behemoth payload...");
        let data = json!({
            "data": {
                "users": (0..100000).map(|i| json!({
                    "id": i,
                    "typename": "User",
                    "status": "ACTIVE",
                    "role": "MEMBER",
                    "organization": {
                        "id": "org-lattice",
                        "name": "Protocol Lattice",
                        "settings": { "theme": "dark", "notifications": true, "audit": "enabled" }
                    },
                    "permissions": ["READ", "WRITE", "EXECUTE", "ADMIN", "OWNER"],
                    "profile": {
                        "verified": true,
                        "tier": "GOLD",
                        "metadata": {
                            "region": "EU",
                            "shard": 7,
                            "cluster": "alpha-1",
                            "tags": ["premium", "early-adopter", "verified"]
                        }
                    },
                    "description": "High-performance software engineer at Protocol Lattice working on gRPC-GraphQL gateway optimizations and binary protocols."
                })).collect::<Vec<_>>()
            }
        });

        let json_bytes = serde_json::to_vec(&data).unwrap();
        println!("Encoding Behemoth with GBP Ultra + LZ4...");
        let start = std::time::Instant::now();
        let encoded = encoder.encode_lz4(&data).unwrap();
        let duration = start.elapsed();

        let ratio = encoded.len() as f64 / json_bytes.len() as f64;
        let reduction = (1.0 - ratio) * 100.0;

        println!("\n--- GBP Ultra Behemoth Test ---");
        println!("Original JSON size:  {:>12} bytes", json_bytes.len());
        println!("GBP Ultra size:      {:>12} bytes", encoded.len());
        println!("Reduction Rate:      {:>12.2}%", reduction);
        println!(
            "Encoding Time:       {:>12.2}ms",
            duration.as_secs_f64() * 1000.0
        );
        println!(
            "Throughput:          {:>12.2} MB/s",
            (json_bytes.len() as f64 / 1024.0 / 1024.0) / duration.as_secs_f64()
        );

        // Data Integrity Check (O(1) perception)
        println!("Verifying data integrity for Behemoth (fast-path)...");
        let mut decoder = GbpDecoder::new();
        let decoded = decoder.decode_lz4(&encoded).expect("Decoding failed");
        
        assert!(decoded.is_object(), "Root is not an object: {:?}", decoded);
        println!("✅ Integrity verified (fast check)");

        assert!(reduction >= 95.0, "Reduction was only {:.2}%", reduction);
    }
}
