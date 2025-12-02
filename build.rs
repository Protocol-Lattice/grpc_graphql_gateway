fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Rebuild if proto or script changes
    println!("cargo:rerun-if-changed=proto/graphql.proto");
    println!("cargo:rerun-if-changed=proto/greeter.proto");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = std::env::var("OUT_DIR")?;
    let out_path = std::path::PathBuf::from(out_dir);

    // Path to google/protobuf/*.proto (provided by prost)
    let proto_include =
        std::env::var("PROTOC_INCLUDE").unwrap_or_else(|_| "/usr/local/include".to_string());
    let proto_paths = ["proto", &proto_include];

    // Remove this line if you don't want the warning:
    // println!("cargo:warning=Using proto include path: {proto_include}");

    tonic_build::configure()
        .build_server(false)
        .build_client(false)
        .file_descriptor_set_path(out_path.join("graphql_descriptor.bin"))
        .compile_protos(&["proto/graphql.proto"], &proto_paths)?;

    // Build the greeter example descriptor + generated code for the example binary
    tonic_build::configure()
        .file_descriptor_set_path(out_path.join("greeter_descriptor.bin"))
        .compile_protos(&["proto/greeter.proto"], &proto_paths)?;

    Ok(())
}
