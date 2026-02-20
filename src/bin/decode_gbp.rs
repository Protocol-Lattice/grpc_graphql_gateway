use grpc_graphql_gateway::gbp::GbpDecoder;
use std::fs;

fn main() {
    let bytes = fs::read("/tmp/gbp_response.bin").expect("Failed to read GBP file");
    println!("📦 GBP Binary: {} bytes", bytes.len());
    println!("🔍 Magic header: {:?}", std::str::from_utf8(&bytes[..3]).unwrap_or("?"));
    println!();
    
    let mut decoder = GbpDecoder::new();
    match decoder.decode(&bytes) {
        Ok(value) => {
            let json = serde_json::to_string_pretty(&value).unwrap();
            let json_size = serde_json::to_string(&value).unwrap().len();
            println!("✅ Decoded GBP ({} bytes) → JSON ({} bytes)", bytes.len(), json_size);
            println!("📊 Compression ratio: {:.1}%", (1.0 - bytes.len() as f64 / json_size as f64) * 100.0);
            println!();
            // Print first 2000 chars to keep output manageable
            if json.len() > 2000 {
                println!("{}", &json[..2000]);
                println!("... ({} more chars)", json.len() - 2000);
            } else {
                println!("{}", json);
            }
        }
        Err(e) => eprintln!("❌ Failed to decode: {}", e),
    }
}
