use serde_json::{Value, Map};
use std::collections::HashMap;
use lz4::EncoderBuilder;
use std::io::{Read, Write, Cursor};

/// GraphQL Binary Protocol (GBP) - ULTRA v8
#[derive(Default)]
pub struct GbpEncoder {
    string_frequencies: HashMap<String, u32>,
    string_pool: Vec<String>,
    string_map: HashMap<String, u32>,
    shape_pool: Vec<Vec<u32>>,
    shape_map: HashMap<Vec<u32>, u32>,
    value_map: HashMap<String, u32>,
    value_counter: u32,
}

impl GbpEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn encode(&mut self, value: &Value) -> Vec<u8> {
        self.analyze_frequencies(value);
        let mut data = Vec::new();
        self.encode_recursive(value, &mut data);

        let mut buf = Vec::new();
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
        let mut encoder = EncoderBuilder::new()
            .level(12) 
            .build(Vec::new())?;
        encoder.write_all(&gbp_data)?;
        let (compressed, result) = encoder.finish();
        result?;
        Ok(compressed)
    }

    fn analyze_frequencies(&mut self, value: &Value) {
        match value {
            Value::String(s) => {
                *self.string_frequencies.entry(s.clone()).or_insert(0) += 1;
            }
            Value::Array(arr) => {
                for v in arr { self.analyze_frequencies(v); }
            }
            Value::Object(obj) => {
                for (k, v) in obj {
                    *self.string_frequencies.entry(k.clone()).or_insert(0) += 2;
                    self.analyze_frequencies(v);
                }
            }
            _ => {}
        }
    }

    fn encode_recursive(&mut self, value: &Value, buf: &mut Vec<u8>) {
        if value.is_object() || value.is_array() {
            let val_key = format!("{}", value);
            if let Some(&idx) = self.value_map.get(&val_key) {
                buf.push(0x08);
                write_varint(idx, buf);
                return;
            }
        }

        match value {
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
                let freq = *self.string_frequencies.get(s).unwrap_or(&0);
                if freq > 1 {
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
            Value::Array(arr) => {
                if self.try_encode_columnar(arr, buf) {
                    // Note: try_encode_columnar already handled recursive calls
                } else {
                    buf.push(0x06);
                    write_varint(arr.len() as u32, buf);
                    for v in arr { self.encode_recursive(v, buf); }
                }
            }
            Value::Object(obj) => {
                let mut keys: Vec<String> = obj.keys().cloned().collect();
                keys.sort();
                let key_indices: Vec<u32> = keys.iter().map(|k| self.get_string_idx(k)).collect();
                let shape_id = self.get_shape_id(key_indices);
                
                buf.push(0x07);
                write_varint(shape_id, buf);
                for k in keys { self.encode_recursive(&obj[&k], buf); }
            }
        }

        // Add to value_map AFTER encoding children (Post-Order)
        if (value.is_object() && value.as_object().unwrap().len() > 1) || 
           (value.is_array() && value.as_array().unwrap().len() > 1) {
            let val_key = format!("{}", value);
            self.value_map.insert(val_key, self.value_counter);
            self.value_counter += 1;
        }
    }

    fn try_encode_columnar(&mut self, arr: &[Value], buf: &mut Vec<u8>) -> bool {
        if arr.len() < 5 { return false; }
        let first = &arr[0];
        if !first.is_object() { return false; }
        let mut keys: Vec<String> = first.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        for item in arr {
            if !item.is_object() || item.as_object().unwrap().len() != keys.len() { return false; }
        }
        buf.push(0x09);
        write_varint(arr.len() as u32, buf);
        let key_indices: Vec<u32> = keys.iter().map(|k| self.get_string_idx(k)).collect();
        let shape_id = self.get_shape_id(key_indices);
        write_varint(shape_id, buf);
        for k in &keys {
            for item in arr { self.encode_recursive(&item[k], buf); }
        }
        true
    }

    fn get_string_idx(&mut self, s: &str) -> u32 {
        if let Some(&idx) = self.string_map.get(s) { idx } else {
            let idx = self.string_pool.len() as u32;
            self.string_pool.push(s.to_string());
            self.string_map.insert(s.to_string(), idx);
            idx
        }
    }

    fn get_shape_id(&mut self, keys: Vec<u32>) -> u32 {
        if let Some(&id) = self.shape_map.get(&keys) { id } else {
            let id = self.shape_pool.len() as u32;
            self.shape_pool.push(keys.clone());
            self.shape_map.insert(keys, id);
            id
        }
    }
}

pub struct GbpDecoder {
    string_pool: Vec<String>,
    shape_pool: Vec<Vec<u32>>,
    value_pool: Vec<Value>,
}

impl GbpDecoder {
    pub fn new() -> Self {
        Self {
            string_pool: Vec::new(),
            shape_pool: Vec::new(),
            value_pool: Vec::new(),
        }
    }

    pub fn decode(&mut self, data: &[u8]) -> Result<Value, String> {
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
            self.string_pool.push(String::from_utf8(buf).map_err(|e| e.to_string())?);
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
        let mut decoder = lz4::Decoder::new(data).map_err(|e| e.to_string())?;
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded).map_err(|e| e.to_string())?;
        self.decode(&decoded)
    }

    fn decode_recursive(&mut self, cursor: &mut Cursor<&[u8]>) -> Result<Value, String> {
        let mut tag = [0u8; 1];
        cursor.read_exact(&mut tag).map_err(|e| e.to_string())?;
        
        if tag[0] == 0x08 {
            let idx = read_varint(cursor)?;
            return self.value_pool.get(idx as usize).cloned().ok_or("Invalid value reference".to_string());
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
                Value::String(self.string_pool.get(idx as usize).cloned().ok_or("Invalid string index")?)
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
                let shape = self.shape_pool.get(shape_id as usize).ok_or("Invalid shape id")?.clone();
                let mut obj = Map::new();
                for key_idx in shape {
                    let key = self.string_pool.get(key_idx as usize).ok_or("Invalid key index")?.clone();
                    let val = self.decode_recursive(cursor)?;
                    obj.insert(key, val);
                }
                Value::Object(obj)
            }
            0x09 => {
                let len = read_varint(cursor)?;
                let shape_id = read_varint(cursor)?;
                let shape = self.shape_pool.get(shape_id as usize).ok_or("Invalid shape id")?.clone();
                let mut arr = vec![Map::new(); len as usize];
                for key_idx in shape {
                    let key = self.string_pool.get(key_idx as usize).ok_or("Invalid key index")?.clone();
                    for i in 0..len {
                        let val = self.decode_recursive(cursor)?;
                        arr[i as usize].insert(key.clone(), val);
                    }
                }
                Value::Array(arr.into_iter().map(Value::Object).collect())
            }
            0x0A => {
                let len = read_varint(cursor)?;
                let mut buf = vec![0u8; len as usize];
                cursor.read_exact(&mut buf).map_err(|e| e.to_string())?;
                Value::String(String::from_utf8(buf).map_err(|e| e.to_string())?)
            }
            _ => return Err(format!("Unknown tag: 0x{:02X}", tag[0])),
        };

        // Sync with encoder's value_map logic: only pool complex structures
        if (value.is_object() && value.as_object().unwrap().len() > 1) || 
           (value.is_array() && value.as_array().unwrap().len() > 1) {
            self.value_pool.push(value.clone());
        }

        Ok(value)
    }
}

fn write_varint(n: u32, buf: &mut Vec<u8>) {
    let mut val = n;
    while val >= 0x80 { buf.push((val & 0x7F) as u8 | 0x80); val >>= 7; }
    buf.push(val as u8);
}

fn read_varint(cursor: &mut Cursor<&[u8]>) -> Result<u32, String> {
    let mut res = 0u32;
    let mut shift = 0;
    loop {
        let mut b = [0u8; 1];
        cursor.read_exact(&mut b).map_err(|e| e.to_string())?;
        res |= ((b[0] & 0x7F) as u32) << shift;
        if b[0] & 0x80 == 0 { break; }
        shift += 7;
    }
    Ok(res)
}

fn write_varint_i64(n: i64, buf: &mut Vec<u8>) {
    let val = ((n << 1) ^ (n >> 63)) as u64;
    let mut val = val;
    while val >= 0x80 { buf.push((val & 0x7F) as u8 | 0x80); val >>= 7; }
    buf.push(val as u8);
}

fn read_varint_i64(cursor: &mut Cursor<&[u8]>) -> Result<i64, String> {
    let mut val = 0u64;
    let mut shift = 0;
    loop {
        let mut b = [0u8; 1];
        cursor.read_exact(&mut b).map_err(|e| e.to_string())?;
        val |= ((b[0] & 0x7F) as u64) << shift;
        if b[0] & 0x80 == 0 { break; }
        shift += 7;
    }
    Ok(((val >> 1) as i64) ^ -((val & 1) as i64))
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
}
