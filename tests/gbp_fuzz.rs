use grpc_graphql_gateway::gbp::{GbpDecoder, GbpEncoder};
use proptest::prelude::*;
use serde_json::{Map, Value};

// Strategy for generating arbitrary JSON values
// We limit depth and size to avoid stack overflows/timeouts during test generation
fn json_strategy() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        // We use typical float values, avoiding NaN/Infinity for simpler equality checks if possible,
        // though JSON spec allows some flexibility. serde_json usually handles sensible floats.
        any::<f64>().prop_map(|n| {
            if n.is_finite() {
                serde_json::Number::from_f64(n)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            }
        }),
        any::<String>().prop_map(Value::String),
        any::<i64>().prop_map(|n| Value::Number(n.into())),
    ];

    leaf.prop_recursive(
        4,  // Depth
        64, // Max size
        10, // Items per collection
        |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
                prop::collection::hash_map(".*", inner, 0..5).prop_map(|m| {
                    let mut map = Map::new();
                    for (k, v) in m {
                        map.insert(k, v);
                    }
                    Value::Object(map)
                }),
            ]
        },
    )
}

proptest! {
    // Run more cases for better coverage
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn test_gbp_roundtrip_correctness(val in json_strategy()) {
        let mut encoder = GbpEncoder::new();
        let encoded = encoder.encode(&val);

        let mut decoder = GbpDecoder::new();
        let decoded_result = decoder.decode(&encoded);

        // 1. Should always decode successfully
        prop_assert!(decoded_result.is_ok(), "Failed to decode valid encoded data: {:?}", decoded_result.err());
        let decoded = decoded_result.unwrap();

        // 2. Should match original (ignoring float precision issues if any, but serde_json is usually consistent)
        // Note: Floating point equality is tricky. We accept that identical floats match.
        // If this fails due to float slight diffs, we might need a custom assertion, but for now strict eq:
        prop_assert_eq!(&val, &decoded, "Decoded value does not match original");
    }

    #[test]
    fn test_gbp_no_panic_on_random_bytes(data in prop::collection::vec(any::<u8>(), 0..1024)) {
        let mut decoder = GbpDecoder::new();
        // The decoder should return Ok or Err, NEVER panic.
        let _ = decoder.decode(&data);
    }

    #[test]
    fn test_gbp_no_panic_on_corrupted_data(
        val in json_strategy(),
        offset in any::<usize>(),
        byte in any::<u8>()
    ) {
        let mut encoder = GbpEncoder::new();
        let mut encoded = encoder.encode(&val);

        if !encoded.is_empty() {
            // Apply single byte corruption
            let idx = offset % encoded.len();
            encoded[idx] = byte;

            let mut decoder = GbpDecoder::new();
            // Should not panic, but likely return Err
            let _ = decoder.decode(&encoded);
        }
    }

    #[test]
    fn test_gbp_no_panic_on_truncation(
        val in json_strategy(),
        len_sub in any::<usize>()
    ) {
        let mut encoder = GbpEncoder::new();
        let mut encoded = encoder.encode(&val);

        if !encoded.is_empty() {
            // Truncate the buffer
            let cut = len_sub % encoded.len();
            encoded.truncate(encoded.len() - cut);

            let mut decoder = GbpDecoder::new();
            // Should not panic, likely Err("failed to fill whole buffer" or similar)
            let _ = decoder.decode(&encoded);
        }
    }
}
