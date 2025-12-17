//! protoc plugin that emits a starter GraphQL gateway for the services in the
//! provided `.proto` files. The output is a single `graphql_gateway.rs` file
//! that you can drop into your project and fill in service endpoints.

use grpc_graphql_gateway::graphql::{GraphqlEntity, GraphqlSchema, GraphqlService, GraphqlType};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, ExtensionDescriptor, Value};
use prost_types::compiler::{code_generator_response, CodeGeneratorResponse};

use std::collections::HashSet;
use std::io::{Read, Write};

#[derive(Debug, Clone)]
struct MethodInfo {
    name: String,
    proto_name: String,
    input_type: String,
    output_type: String,
    client_streaming: bool,
    server_streaming: bool,
}

/// Collected service metadata for template generation.
#[derive(Debug, Clone)]
struct ServiceInfo {
    /// Fully-qualified service name (e.g. package.Service)
    full_name: String,
    /// Optional endpoint from `(graphql.service)` option
    endpoint: Option<String>,
    /// Whether to connect insecurely (defaults to true if not provided)
    insecure: bool,
    ops: OperationBuckets,
    methods: Vec<MethodInfo>,
}

/// Metadata about federated entity types (graphql.entity).
#[derive(Debug, Clone)]
struct EntityInfo {
    /// GraphQL type name (dots replaced with underscores)
    type_name: String,
    /// Fully qualified protobuf message name
    full_name: String,
    /// Key field sets
    keys: Vec<Vec<String>>,
    extend: bool,
    resolvable: bool,
}

#[derive(Debug, Default, Clone)]
struct OperationBuckets {
    queries: Vec<String>,
    mutations: Vec<String>,
    subscriptions: Vec<String>,
    resolvers: Vec<String>,
}

#[derive(Default)]
struct TemplateOptions {
    descriptor_path: Option<String>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
struct RawCodeGeneratorRequest {
    #[prost(string, repeated, tag = "1")]
    pub file_to_generate: ::prost::alloc::vec::Vec<String>,
    #[prost(string, optional, tag = "2")]
    pub parameter: Option<String>,
    #[prost(bytes, repeated, tag = "15")]
    pub proto_file: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
struct RawFileDescriptorSet {
    #[prost(bytes, repeated, tag = "1")]
    pub file: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Read CodeGeneratorRequest from stdin
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    let request = RawCodeGeneratorRequest::decode(&*input)?;

    let options = parse_options(request.parameter.as_deref());
    let pool = build_descriptor_pool(&request)?;
    let services = collect_services(&pool, &request)?;
    let entities = collect_entities(&pool, &request)?;
    let content = render_template(&services, &entities, &request.file_to_generate, &options);

    let filename = "./src/main.rs".to_string();

    let response = CodeGeneratorResponse {
        file: vec![code_generator_response::File {
            name: Some(filename),
            insertion_point: None,
            content: Some(content),
            generated_code_info: None,
        }],
        ..Default::default()
    };

    let mut output = Vec::new();
    response.encode(&mut output)?;
    std::io::stdout().write_all(&output)?;
    Ok(())
}

fn parse_options(param: Option<&str>) -> TemplateOptions {
    let mut opts = TemplateOptions::default();
    let Some(param) = param else {
        return opts;
    };

    for part in param.split(',').map(|p| p.trim()).filter(|p| !p.is_empty()) {
        if let Some(rest) = part.strip_prefix("descriptor_path=") {
            opts.descriptor_path = Some(rest.to_string());
        }
    }

    opts
}

fn build_descriptor_pool(
    request: &RawCodeGeneratorRequest,
) -> Result<DescriptorPool, Box<dyn std::error::Error>> {
    let fds = RawFileDescriptorSet {
        file: request.proto_file.clone(),
    };
    let mut bytes = Vec::new();
    fds.encode(&mut bytes)?;
    DescriptorPool::decode(bytes.as_slice()).map_err(|e| e.into())
}

fn is_target_file(targets: &HashSet<&str>, file_name: &str) -> bool {
    if targets.contains(file_name) {
        return true;
    }

    for t in targets {
        if t.ends_with(file_name) || file_name.ends_with(*t) {
            return true;
        }

        // Normalize slashes for windows/linux paths
        let t_norm = t.replace('\\', "/");
        let f_norm = file_name.replace('\\', "/");
        if t_norm.ends_with(&f_norm) || f_norm.ends_with(&t_norm) {
            return true;
        }
    }

    false
}

/// Collect the services from the files protoc asked us to generate for, along with
/// GraphQL operations (queries, mutations, subscriptions, resolvers).
fn collect_services(
    pool: &DescriptorPool,
    request: &RawCodeGeneratorRequest,
) -> Result<Vec<ServiceInfo>, Box<dyn std::error::Error>> {
    let targets: HashSet<&str> = request
        .file_to_generate
        .iter()
        .map(|s| s.as_str())
        .collect();

    let method_ext = pool.get_extension_by_name("graphql.schema");
    let service_ext = pool.get_extension_by_name("graphql.service");

    let mut services = Vec::new();
    for svc in pool.services() {
        let parent_file = svc.parent_file();
        let file_name = parent_file.name();

        if !is_target_file(&targets, file_name) {
            continue;
        }

        let mut info = ServiceInfo {
            full_name: svc.full_name().to_string(),
            endpoint: None,
            insecure: true,
            ops: OperationBuckets::default(),
            methods: Vec::new(),
        };

        if let Some(ext) = service_ext.as_ref() {
            if let Some(opts) = decode_extension::<GraphqlService>(&svc.options(), ext)? {
                if !opts.host.is_empty() {
                    info.endpoint = Some(opts.host);
                }
                info.insecure = opts.insecure;
            }
        }

        for method in svc.methods() {
            info.methods.push(MethodInfo {
                name: to_snake_case(method.name()),
                proto_name: method.name().to_string(),
                input_type: resolve_type(method.input()),
                output_type: resolve_type(method.output()),
                client_streaming: method.is_client_streaming(),
                server_streaming: method.is_server_streaming(),
            });

            if let Some(method_ext) = method_ext.as_ref() {
                let Some(schema_opts) =
                    decode_extension::<GraphqlSchema>(&method.options(), method_ext)?
                else {
                    continue;
                };

                let graphql_name = if schema_opts.name.is_empty() {
                    method.name().to_string()
                } else {
                    schema_opts.name.clone()
                };

                match GraphqlType::try_from(schema_opts.r#type).unwrap_or(GraphqlType::Query) {
                    GraphqlType::Query => info.ops.queries.push(graphql_name),
                    GraphqlType::Mutation => info.ops.mutations.push(graphql_name),
                    GraphqlType::Subscription => info.ops.subscriptions.push(graphql_name),
                    GraphqlType::Resolver => info.ops.resolvers.push(graphql_name),
                }
            }
        }

        services.push(info);
    }

    services.sort_by(|a, b| a.full_name.cmp(&b.full_name));
    Ok(services)
}

/// Collect entity definitions for federation support.
fn collect_entities(
    pool: &DescriptorPool,
    request: &RawCodeGeneratorRequest,
) -> Result<Vec<EntityInfo>, Box<dyn std::error::Error>> {
    let targets: HashSet<&str> = request
        .file_to_generate
        .iter()
        .map(|s| s.as_str())
        .collect();

    let Some(entity_ext) = pool.get_extension_by_name("graphql.entity") else {
        return Ok(Vec::new());
    };

    let mut entities = Vec::new();
    for msg in pool.all_messages() {
        let parent_file = msg.parent_file();
        let file_name = parent_file.name();
        if !is_target_file(&targets, file_name) {
            continue;
        }

        let Some(opts) = decode_extension::<GraphqlEntity>(&msg.options(), &entity_ext)? else {
            continue;
        };

        if opts.keys.is_empty() {
            continue;
        }

        let keys = opts
            .keys
            .iter()
            .map(|key| {
                key.split_whitespace()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        entities.push(EntityInfo {
            type_name: msg.full_name().replace('.', "_"),
            full_name: msg.full_name().to_string(),
            keys,
            extend: opts.extend,
            resolvable: opts.resolvable,
        });
    }

    entities.sort_by(|a, b| a.type_name.cmp(&b.type_name));
    Ok(entities)
}

fn decode_extension<T: Message + Default>(
    opts: &DynamicMessage,
    ext: &ExtensionDescriptor,
) -> Result<Option<T>, Box<dyn std::error::Error>> {
    if !opts.has_extension(ext) {
        return Ok(None);
    }

    let val = opts.get_extension(ext);
    if let Value::Message(msg) = val.as_ref() {
        return T::decode(msg.encode_to_vec().as_slice())
            .map(Some)
            .map_err(|e| e.into());
    }

    Ok(None)
}

/// Render the Rust template that wires the gateway together.
fn render_template(
    services: &[ServiceInfo],
    entities: &[EntityInfo],
    files: &[String],
    options: &TemplateOptions,
) -> String {
    let all_queries = collect_ops(services, |ops| &ops.queries);
    let all_mutations = collect_ops(services, |ops| &ops.mutations);
    let all_subscriptions = collect_ops(services, |ops| &ops.subscriptions);
    let all_resolvers = collect_ops(services, |ops| &ops.resolvers);
    let has_federation = !entities.is_empty();
    let entity_resolver_name = if has_federation {
        derive_entity_resolver_name(entities, services)
    } else {
        String::new()
    };

    let mut buf = String::new();
    buf.push_str("// @generated by protoc-gen-graphql-template\n");
    buf.push_str("// Source files:\n");
    for file in files {
        buf.push_str(&format!("//   - {file}\n"));
    }
    buf.push_str("//\n");
    buf.push_str("// This is a starter gateway. Update endpoint URLs and tweak as needed.\n\n");

    buf.push_str("use grpc_graphql_gateway::{Gateway, GatewayBuilder, GrpcClient, Result as GatewayResult};\n");
    if has_federation {
        buf.push_str("use async_graphql::{Name, Value as GqlValue};\n");
        buf.push_str("use std::sync::Arc;\n");
    }
    buf.push_str("use std::net::{SocketAddr, ToSocketAddrs};\n");
    buf.push_str("use std::pin::Pin;\n");
    buf.push_str("use tonic::{transport::Server, Request, Response, Status};\n");
    buf.push_str("use tracing_subscriber::prelude::*;\n\n");
    buf.push_str("type ServiceResult<T> = std::result::Result<T, Status>;\n\n");
    let descriptor_expr = options
        .descriptor_path
        .as_deref()
        .map(render_str_literal)
        .unwrap_or_else(|| render_str_literal(&default_descriptor_path(services, files)));
    buf.push_str(&format!(
        "const DESCRIPTOR_SET: &[u8] = include_bytes!({descriptor_expr});\n\n"
    ));
    buf.push_str("const DEFAULT_GRPC_ADDR: &str = \"0.0.0.0:50051\";\n\n");

    buf.push_str("fn describe(list: &[&str]) -> String {\n");
    buf.push_str("    if list.is_empty() { \"none\".to_string() } else { list.join(\", \") }\n");
    buf.push_str("}\n\n");
    buf.push_str("fn describe_resolvers(list: &[&str]) -> String {\n");
    buf.push_str("    if list.is_empty() { \"none\".to_string() } else { list.join(\", \") }\n");
    buf.push_str("}\n\n");
    buf.push_str("fn listen_addr(endpoint: &str, fallback: &str) -> GatewayResult<SocketAddr> {\n");
    buf.push_str("    let mut addr = endpoint.trim();\n");
    buf.push_str("    if let Some(stripped) = addr.strip_prefix(\"http://\").or_else(|| addr.strip_prefix(\"https://\")) {\n");
    buf.push_str("        addr = stripped;\n");
    buf.push_str("    }\n");
    buf.push_str("    if let Some((host, _rest)) = addr.split_once('/') {\n");
    buf.push_str("        addr = host;\n");
    buf.push_str("    }\n");
    buf.push_str("    if let Ok(sock) = addr.parse() {\n");
    buf.push_str("        return Ok(sock);\n");
    buf.push_str("    }\n");
    buf.push_str("    if let Ok(mut iter) = addr.to_socket_addrs() {\n");
    buf.push_str("        if let Some(sock) = iter.next() {\n");
    buf.push_str("            return Ok(sock);\n");
    buf.push_str("        }\n");
    buf.push_str("    }\n");
    buf.push_str("    fallback\n");
    buf.push_str("        .parse()\n");
    buf.push_str(
        "        .map_err(|e| grpc_graphql_gateway::Error::Other(anyhow::Error::new(e)))\n",
    );
    buf.push_str("}\n\n");
    if has_federation {
        buf.push_str("fn describe_key_sets(keys: &[&[&str]]) -> String {\n");
        buf.push_str("    if keys.is_empty() {\n");
        buf.push_str("        \"none\".to_string()\n");
        buf.push_str("    } else {\n");
        buf.push_str("        keys\n");
        buf.push_str("            .iter()\n");
        buf.push_str("            .map(|set| set.join(\" \"))\n");
        buf.push_str("            .collect::<Vec<_>>()\n");
        buf.push_str("            .join(\" | \")\n");
        buf.push_str("    }\n");
        buf.push_str("}\n\n");
        buf.push_str("fn describe_entities() -> String {\n");
        buf.push_str("    if ENTITY_CONFIGS.is_empty() {\n");
        buf.push_str("        \"none\".to_string()\n");
        buf.push_str("    } else {\n");
        buf.push_str("        ENTITY_CONFIGS\n");
        buf.push_str("            .iter()\n");
        buf.push_str("            .map(|e| format!(\"{} (keys: {})\", e.type_name, describe_key_sets(e.keys)))\n");
        buf.push_str("            .collect::<Vec<_>>()\n");
        buf.push_str("            .join(\", \")\n");
        buf.push_str("    }\n");
        buf.push_str("}\n\n");
    }

    buf.push_str(&format!(
        "const QUERIES: &[&str] = {};\n",
        render_str_slice(&all_queries)
    ));
    buf.push_str(&format!(
        "const MUTATIONS: &[&str] = {};\n",
        render_str_slice(&all_mutations)
    ));
    buf.push_str(&format!(
        "const SUBSCRIPTIONS: &[&str] = {};\n",
        render_str_slice(&all_subscriptions)
    ));
    buf.push_str(&format!(
        "const RESOLVERS: &[&str] = {};\n",
        render_resolvers_slice(&all_resolvers, has_federation)
    ));
    buf.push_str("#[allow(dead_code)]\n");
    buf.push_str(&format!(
        "const FEDERATION_ENABLED: bool = {};\n\n",
        has_federation
    ));
    if has_federation {
        buf.push_str("pub struct EntityConfigInfo {\n");
        buf.push_str("    pub type_name: &'static str,\n");
        buf.push_str("    pub keys: &'static [&'static [&'static str]],\n");
        buf.push_str("    pub extend: bool,\n");
        buf.push_str("    pub resolvable: bool,\n");
        buf.push_str("}\n\n");
        buf.push_str(&format!(
            "pub const ENTITY_CONFIGS: &[EntityConfigInfo] = {};\n\n",
            render_entity_configs(entities)
        ));
    }
    buf.push_str("\n");

    // Generate modules for the proto packages
    let mut packages = std::collections::BTreeSet::new();
    for svc in services {
        if let Some((pkg, _)) = svc.full_name.rsplit_once('.') {
            packages.insert(pkg);
        }
    }

    for pkg in packages {
        let mod_name = pkg.replace('.', "_");
        buf.push_str(&format!("pub mod {} {{\n", mod_name));
        buf.push_str(&format!("    include!(\"./{}.rs\");\n", pkg));
        buf.push_str("}\n\n");
    }

    // Generate use statements for the service traits
    for svc in services {
        if let Some((pkg, svc_name)) = svc.full_name.rsplit_once('.') {
            let mod_name = pkg.replace('.', "_");
            let server_mod = format!("{}_server", to_snake_case(svc_name));
            buf.push_str(&format!(
                "use {}::{}::{{{}, {}Server}};\n",
                mod_name, server_mod, svc_name, svc_name
            ));
        }
    }
    buf.push_str("\n");

    buf.push_str("pub struct ServiceConfig {\n");
    buf.push_str("    pub name: &'static str,\n");
    buf.push_str("    pub endpoint: &'static str,\n");
    buf.push_str("    pub insecure: bool,\n");
    buf.push_str("    pub queries: &'static [&'static str],\n");
    buf.push_str("    pub mutations: &'static [&'static str],\n");
    buf.push_str("    pub subscriptions: &'static [&'static str],\n");
    buf.push_str("    pub resolvers: &'static [&'static str],\n");
    buf.push_str("}\n\n");

    buf.push_str("pub mod services {\n");
    buf.push_str("    use super::ServiceConfig;\n\n");

    let mut service_consts = Vec::new();
    for (idx, svc) in services.iter().enumerate() {
        let const_name = svc.full_name.replace('.', "_").to_uppercase();
        let endpoint = normalize_endpoint(
            &svc
                .endpoint
                .clone()
                .unwrap_or_else(|| default_service_endpoint(idx)),
        );
        service_consts.push(const_name.clone());

        buf.push_str(&format!(
            "    pub const {}: ServiceConfig = ServiceConfig {{\n",
            const_name
        ));
        buf.push_str(&format!(
            "        name: {},\n",
            render_str_literal(&svc.full_name)
        ));
        buf.push_str(&format!(
            "        endpoint: {},\n",
            render_str_literal(&endpoint)
        ));
        buf.push_str(&format!("        insecure: {},\n", svc.insecure));
        buf.push_str(&format!(
            "        queries: {},\n",
            render_str_slice(&svc.ops.queries)
        ));
        buf.push_str(&format!(
            "        mutations: {},\n",
            render_str_slice(&svc.ops.mutations)
        ));
        buf.push_str(&format!(
            "        subscriptions: {},\n",
            render_str_slice(&svc.ops.subscriptions)
        ));
        buf.push_str(&format!(
            "        resolvers: {},\n",
            render_resolvers_slice(&svc.ops.resolvers, has_federation)
        ));
        buf.push_str("    };\n");
    }

    buf.push_str("\n    pub const ALL: &[ServiceConfig] = &[\n");
    for name in service_consts {
        buf.push_str(&format!("        {},\n", name));
    }
    buf.push_str("    ];\n");
    buf.push_str("}\n\n");

    if has_federation {
        buf.push_str("/// Example entity resolver stub for federation. Replace with your own logic or DataLoader.\n");
        buf.push_str("#[derive(Clone, Default)]\n");
        buf.push_str(&format!("pub struct {};\n\n", entity_resolver_name));
        buf.push_str("#[async_trait::async_trait]\n");
        buf.push_str(&format!(
            "impl grpc_graphql_gateway::EntityResolver for {} {{\n",
            entity_resolver_name
        ));
        buf.push_str("    async fn resolve_entity(\n");
        buf.push_str("        &self,\n");
        buf.push_str("        entity_config: &grpc_graphql_gateway::federation::EntityConfig,\n");
        buf.push_str(
            "        representation: &async_graphql::indexmap::IndexMap<Name, GqlValue>,\n",
        );
        buf.push_str("    ) -> grpc_graphql_gateway::Result<GqlValue> {\n");
        buf.push_str("        let mut obj = representation.clone();\n");
        buf.push_str("        obj.shift_remove(&Name::new(\"__typename\"));\n");
        buf.push_str("        Ok(GqlValue::Object(obj))\n");
        buf.push_str("    }\n\n");
        buf.push_str("    async fn batch_resolve_entities(\n");
        buf.push_str("        &self,\n");
        buf.push_str("        entity_config: &grpc_graphql_gateway::federation::EntityConfig,\n");
        buf.push_str(
            "        representations: Vec<async_graphql::indexmap::IndexMap<Name, GqlValue>>,\n",
        );
        buf.push_str("    ) -> grpc_graphql_gateway::Result<Vec<GqlValue>> {\n");
        buf.push_str("        let mut results = Vec::with_capacity(representations.len());\n");
        buf.push_str("        for repr in representations {\n");
        buf.push_str(
            "            results.push(self.resolve_entity(entity_config, &repr).await?);\n",
        );
        buf.push_str("        }\n");
        buf.push_str("        Ok(results)\n");
        buf.push_str("    }\n");
        buf.push_str("}\n\n");
        buf.push_str(
            "fn default_entity_resolver() -> Arc<dyn grpc_graphql_gateway::EntityResolver> {\n",
        );
        buf.push_str(&format!(
            "    Arc::new({}::default())\n",
            entity_resolver_name
        ));
        buf.push_str("}\n\n");
    }

    // Scaffolding for service implementation
    buf.push_str("/// Scaffolding for gRPC service implementations.\n");
    buf.push_str("#[derive(Default, Clone)]\n");
    buf.push_str("pub struct ServiceImpl;\n\n");

    for svc in services {
        let (pkg, svc_name) = svc
            .full_name
            .rsplit_once('.')
            .unwrap_or(("", &svc.full_name));

        let mod_name = pkg.replace('.', "_");
        let server_mod = format!("{}_server", to_snake_case(svc_name));
        let trait_name = svc_name;

        let trait_path = if mod_name.is_empty() {
            format!("{}::{}", server_mod, trait_name)
        } else {
            format!("{}::{}::{}", mod_name, server_mod, trait_name)
        };

        buf.push_str("#[tonic::async_trait]\n");
        buf.push_str(&format!("impl {} for ServiceImpl {{\n", trait_path));

        // Define associated types for streaming response methods
        for method in &svc.methods {
            if method.server_streaming {
                let stream_type = format!("{}Stream", method.proto_name);
                buf.push_str(&format!("    type {} = Pin<Box<dyn futures::Stream<Item = ServiceResult<{}>> + Send>>;\n", stream_type, method.output_type));
            }
        }

        for method in &svc.methods {
            let input_arg = if method.client_streaming {
                format!("tonic::Streaming<{}>", method.input_type)
            } else {
                method.input_type.clone()
            };

            let return_type = if method.server_streaming {
                format!("Self::{}Stream", method.proto_name)
            } else {
                method.output_type.clone()
            };

            buf.push_str(&format!(
                "    async fn {}(&self, _request: Request<{}>) -> ServiceResult<Response<{}>> {{\n",
                method.name, input_arg, return_type
            ));
            buf.push_str("        Err(Status::unimplemented(\"method not implemented\"))\n");
            buf.push_str("    }\n");
        }
        buf.push_str("}\n\n");
    }

    buf.push_str("pub async fn run_services() -> GatewayResult<()> {\n");
    if services.is_empty() {
        buf.push_str("    // No services discovered; nothing to run.\n");
        buf.push_str("    Ok(())\n");
        buf.push_str("}\n\n");
    } else {
        buf.push_str("    let mut handles = Vec::new();\n");
        for (idx, svc) in services.iter().enumerate() {
            let (pkg, svc_name) = svc
                .full_name
                .rsplit_once('.')
                .unwrap_or(("", &svc.full_name));

            let mod_name = pkg.replace('.', "_");
            let server_mod = format!("{}_server", to_snake_case(svc_name));
            let server_struct = format!("{}Server", svc_name);
            let endpoint = svc
                .endpoint
                .clone()
                .unwrap_or_else(|| default_service_endpoint(idx));

            buf.push_str("    {\n");
            buf.push_str(&format!(
                "        let addr: SocketAddr = listen_addr({}, DEFAULT_GRPC_ADDR)?;\n",
                render_str_literal(&endpoint)
            ));
            buf.push_str("        let service = ServiceImpl::default();\n");
            buf.push_str(&format!(
                "        tracing::info!(\"gRPC service {} listening on {{}}\", addr);\n",
                svc.full_name
            ));
            buf.push_str("        let handle = tokio::spawn(async move {\n");
            buf.push_str("            Server::builder()\n");

            if mod_name.is_empty() {
                buf.push_str(&format!(
                    "                .add_service({}::{}::new(service.clone()))\n",
                    server_mod, server_struct
                ));
            } else {
                buf.push_str(&format!(
                    "                .add_service({}::{}::{}::new(service.clone()))\n",
                    mod_name, server_mod, server_struct
                ));
            }

            buf.push_str("                .serve(addr)\n");
            buf.push_str("                .await\n");
            buf.push_str("                .map_err(|e| grpc_graphql_gateway::Error::Other(anyhow::Error::new(e)))\n");
            buf.push_str("        });\n");
            buf.push_str("        handles.push(handle);\n");
            buf.push_str("    }\n");
        }

        buf.push_str("    for handle in handles {\n");
        buf.push_str("        match handle.await {\n");
        buf.push_str("            Ok(Ok(())) => {}\n");
        buf.push_str("            Ok(Err(e)) => {\n");
        buf.push_str("                tracing::warn!(error = %e, \"gRPC service task exited with error\");\n");
        buf.push_str("            }\n");
        buf.push_str("            Err(e) => {\n");
        buf.push_str("                tracing::warn!(error = %e, \"gRPC service task panicked or was cancelled\");\n");
        buf.push_str("            }\n");
        buf.push_str("        }\n");
        buf.push_str("    }\n");
        buf.push_str("    Ok(())\n");
        buf.push_str("}\n\n");
    }

    buf.push_str("pub fn gateway_builder() -> GatewayResult<GatewayBuilder> {\n");
    buf.push_str("    // The descriptor set is produced by your build.rs using tonic-build.\n");
    buf.push_str("    let mut builder = Gateway::builder()\n");
    buf.push_str("        .with_descriptor_set_bytes(DESCRIPTOR_SET);\n\n");
    if has_federation {
        buf.push_str("    if FEDERATION_ENABLED {\n");
        buf.push_str("        tracing::info!(\"Federation enabled (entities: {entities})\", entities = describe_entities());\n");
        buf.push_str("        builder = builder\n");
        buf.push_str("            .enable_federation()\n");
        buf.push_str("            .with_entity_resolver(default_entity_resolver());\n");
        buf.push_str("        // For subgraphs, restrict the schema to the services owned by this process:\n");
        buf.push_str("        // builder = builder.with_services([\"your.package.Service\"]);\n");
        buf.push_str("    }\n\n");
    }

    if services.is_empty() {
        buf.push_str("    // TODO: add gRPC clients. Example:\n");
        buf.push_str("    // builder = builder.add_grpc_client(\n");
        buf.push_str("    //     \"my.package.Service\",\n");
        buf.push_str("    //     GrpcClient::connect_lazy(\"http://127.0.0.1:50051\", true)?,\n");
        buf.push_str("    // );\n");
    } else {
        buf.push_str("    // Add gRPC backends for each service discovered in your protos.\n");
        buf.push_str("    for svc in services::ALL {\n");
        buf.push_str("        tracing::info!(\n");
        buf.push_str("            \"{svc} -> {endpoint} (queries: {queries}; mutations: {mutations}; subscriptions: {subscriptions}; resolvers: {resolvers})\",\n");
        buf.push_str("            svc = svc.name,\n");
        buf.push_str("            endpoint = svc.endpoint,\n");
        buf.push_str("            queries = describe(svc.queries),\n");
        buf.push_str("            mutations = describe(svc.mutations),\n");
        buf.push_str("            subscriptions = describe(svc.subscriptions),\n");
        buf.push_str("            resolvers = describe_resolvers(svc.resolvers),\n");
        buf.push_str("        );\n");
        buf.push_str("        let client = GrpcClient::builder(svc.endpoint)\n");
        buf.push_str("            .insecure(svc.insecure)\n");
        buf.push_str("            .lazy(true)\n");
        buf.push_str("            .connect_lazy()?;\n");
        buf.push_str("        builder = builder.add_grpc_client(svc.name, client);\n");
        buf.push_str("    }\n");
        buf.push_str("\n    // Update the endpoints above to point at your actual services.\n");
    }

    buf.push_str("\n    Ok(builder)\n}\n\n");

    buf.push_str("pub fn gateway_builder_for_service(svc: &ServiceConfig) -> GatewayResult<GatewayBuilder> {\n");
    buf.push_str("    let mut builder = Gateway::builder()\n");
    buf.push_str("        .with_descriptor_set_bytes(DESCRIPTOR_SET);\n\n");
    buf.push_str("    tracing::info!(\n");
    buf.push_str("        \"{svc} -> {endpoint} (queries: {queries}; mutations: {mutations}; subscriptions: {subscriptions}; resolvers: {resolvers})\",\n");
    buf.push_str("        svc = svc.name,\n");
    buf.push_str("        endpoint = svc.endpoint,\n");
    buf.push_str("        queries = describe(svc.queries),\n");
    buf.push_str("        mutations = describe(svc.mutations),\n");
    buf.push_str("        subscriptions = describe(svc.subscriptions),\n");
    buf.push_str("        resolvers = describe_resolvers(svc.resolvers),\n");
    buf.push_str("    );\n\n");
    if has_federation {
        buf.push_str("    if FEDERATION_ENABLED {\n");
        buf.push_str("        builder = builder\n");
        buf.push_str("            .enable_federation()\n");
        buf.push_str("            .with_entity_resolver(default_entity_resolver())\n");
        buf.push_str("            .with_services([svc.name]);\n");
        buf.push_str("    } else {\n");
        buf.push_str("        builder = builder.with_services([svc.name]);\n");
        buf.push_str("    }\n");
    } else {
        buf.push_str("    builder = builder.with_services([svc.name]);\n");
    }
    buf.push_str("\n");
    buf.push_str("    let client = GrpcClient::builder(svc.endpoint)\n");
    buf.push_str("        .insecure(svc.insecure)\n");
    buf.push_str("        .lazy(true)\n");
    buf.push_str("        .connect_lazy()?;\n");
    buf.push_str("    builder = builder.add_grpc_client(svc.name, client);\n");
    buf.push_str("\n");
    buf.push_str("    Ok(builder)\n");
    buf.push_str("}\n\n");

    buf.push_str(
        "pub fn gateway_builder_for(name: &str) -> GatewayResult<Option<GatewayBuilder>> {\n",
    );
    buf.push_str("    for svc in services::ALL {\n");
    buf.push_str("        if svc.name == name {\n");
    buf.push_str("            return gateway_builder_for_service(svc).map(Some);\n");
    buf.push_str("        }\n");
    buf.push_str("    }\n");
    buf.push_str("    Ok(None)\n");
    buf.push_str("}\n\n");

    for (idx, svc) in services.iter().enumerate() {
        let fn_name = format!(
            "run_{}_gateway",
            svc.full_name.replace('.', "_").to_ascii_lowercase()
        );
        let bind_addr = format!("\"0.0.0.0:{}\"", 9000 + idx as u16);

        buf.push_str(&format!(
            "pub async fn {}() -> GatewayResult<()> {{\n",
            fn_name
        ));
        buf.push_str(&format!(
            "    gateway_builder_for_service(&services::{})?\n",
            svc.full_name.replace('.', "_").to_uppercase()
        ));
        buf.push_str(&format!("        .serve({})\n", bind_addr));
        buf.push_str("        .await\n");
        buf.push_str("}\n\n");
    }

    buf.push_str("#[tokio::main]\n");
    buf.push_str("async fn main() -> GatewayResult<()> {\n");
    buf.push_str("    // Basic logging; adjust as desired.\n");
    buf.push_str("    tracing_subscriber::registry()\n");
    buf.push_str("        .with(tracing_subscriber::fmt::layer())\n");
    buf.push_str("        .init();\n\n");
    buf.push_str("    tracing::info!(\n");
    buf.push_str("        \"GraphQL operations -> queries: {queries}; mutations: {mutations}; subscriptions: {subscriptions}; resolvers: {resolvers}\",\n");
    buf.push_str("        queries = describe(QUERIES),\n");
    buf.push_str("        mutations = describe(MUTATIONS),\n");
    buf.push_str("        subscriptions = describe(SUBSCRIPTIONS),\n");
    buf.push_str("        resolvers = describe_resolvers(RESOLVERS),\n");
    buf.push_str("    );\n\n");
    if has_federation {
        buf.push_str("    if FEDERATION_ENABLED {\n");
        buf.push_str("        tracing::info!(\"Federation entities -> {entities}\", entities = describe_entities());\n");
        buf.push_str("    }\n\n");
    }
    buf.push_str("    // NOTE: Resolver entries are listed above; the runtime currently warns that they are not implemented.\n");
    buf.push_str("    // Spawn stub gRPC services and per-service gateways in this process:\n");
    buf.push_str("    tokio::spawn(async {\n");
    buf.push_str("        if let Err(e) = run_services().await {\n");
    buf.push_str("            tracing::error!(error = %e, \"gRPC services exited with error\");\n");
    buf.push_str("        }\n");
    buf.push_str("    });\n\n");
    for svc in services {
        let fn_name = format!(
            "run_{}_gateway",
            svc.full_name.replace('.', "_").to_ascii_lowercase()
        );
        buf.push_str("    tokio::spawn(async {\n");
        buf.push_str(&format!("        if let Err(e) = {}().await {{\n", fn_name));
        buf.push_str(&format!(
            "            tracing::error!(error = %e, \"GraphQL gateway for {} exited with error\");\n",
            svc.full_name
        ));
        buf.push_str("        }\n");
        buf.push_str("    });\n\n");
    }
    buf.push_str("    // Keep running until interrupted so the spawned servers stay alive.\n");
    buf.push_str("    tokio::signal::ctrl_c()\n");
    buf.push_str("        .await\n");
    buf.push_str(
        "        .map_err(|e| grpc_graphql_gateway::Error::Other(anyhow::Error::new(e)))?;\n",
    );
    buf.push_str("\n");
    buf.push_str("    Ok(())\n");
    buf.push_str("}\n");

    buf
}

fn default_descriptor_path(services: &[ServiceInfo], files: &[String]) -> String {
    if let Some(pkg) = services
        .iter()
        .find_map(|svc| svc.full_name.rsplit_once('.').map(|(pkg, _)| pkg))
        .filter(|pkg| !pkg.is_empty())
    {
        return format!("./{}_descriptor.bin", pkg.replace('.', "_"));
    }

    if let Some(stem) = files.iter().find_map(|file| {
        std::path::Path::new(file)
            .file_stem()
            .and_then(|s| s.to_str())
    }) {
        return format!("./{}_descriptor.bin", stem);
    }

    "./descriptor.bin".to_string()
}

fn render_str_literal(input: &str) -> String {
    format!("\"{}\"", input.escape_default())
}

fn render_str_slice(values: &[String]) -> String {
    if values.is_empty() {
        "&[]".to_string()
    } else {
        let joined = values
            .iter()
            .map(|v| render_str_literal(v))
            .collect::<Vec<_>>()
            .join(", ");
        format!("&[{joined}]")
    }
}

fn render_resolvers_slice(values: &[String], include_entities: bool) -> String {
    if !include_entities {
        return render_str_slice(values);
    }

    let mut combined = values.to_vec();
    combined.push("_entities".to_string());
    // remove duplicates while preserving order
    combined.dedup();
    render_str_slice(&combined)
}

fn render_entity_configs(entities: &[EntityInfo]) -> String {
    if entities.is_empty() {
        "&[]".to_string()
    } else {
        let mut buf = String::from("&[\n");
        for entity in entities {
            buf.push_str("    EntityConfigInfo {\n");
            buf.push_str(&format!(
                "        type_name: {},\n",
                render_str_literal(&entity.type_name)
            ));
            buf.push_str(&format!(
                "        keys: {},\n",
                render_key_sets(&entity.keys)
            ));
            buf.push_str(&format!("        extend: {},\n", entity.extend));
            buf.push_str(&format!("        resolvable: {},\n", entity.resolvable));
            buf.push_str("    },\n");
        }
        buf.push(']');
        buf
    }
}

fn default_service_endpoint(idx: usize) -> String {
    format!("http://127.0.0.1:{}", 50051 + idx as u16)
}

fn normalize_endpoint(endpoint: &str) -> String {
    if endpoint.contains("://") {
        endpoint.to_string()
    } else {
        format!("http://{}", endpoint)
    }
}

fn render_key_sets(keys: &[Vec<String>]) -> String {
    if keys.is_empty() {
        "&[]".to_string()
    } else {
        let rendered_sets: Vec<String> = keys
            .iter()
            .map(|set| {
                let inner = set
                    .iter()
                    .map(|k| render_str_literal(k))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("&[{inner}]")
            })
            .collect();
        format!("&[{}]", rendered_sets.join(", "))
    }
}

fn derive_entity_resolver_name(entities: &[EntityInfo], services: &[ServiceInfo]) -> String {
    if let Some(pkg) = entities
        .iter()
        .filter_map(|e| e.full_name.rsplit_once('.').map(|(pkg, _)| pkg))
        .find(|pkg| !pkg.is_empty())
    {
        return format!("{}EntityResolver", to_pascal_case(pkg));
    }

    if let Some(svc) = services.first() {
        if let Some((pkg, _)) = svc.full_name.rsplit_once('.') {
            if !pkg.is_empty() {
                return format!("{}EntityResolver", to_pascal_case(pkg));
            }
        }

        let name = svc.full_name.split('.').last().unwrap_or(&svc.full_name);
        return format!("{}EntityResolver", to_pascal_case(name));
    }

    "ExampleEntityResolver".to_string()
}

fn collect_ops<F>(services: &[ServiceInfo], f: F) -> Vec<String>
where
    F: Fn(&OperationBuckets) -> &Vec<String>,
{
    let mut set = std::collections::BTreeSet::new();
    for svc in services {
        for op in f(&svc.ops) {
            set.insert(op.clone());
        }
    }
    set.into_iter().collect()
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

fn to_pascal_case(input: &str) -> String {
    input
        .split(|c: char| c == '.' || c == '_' || c == '-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<String>()
}

fn resolve_type(msg: prost_reflect::MessageDescriptor) -> String {
    let parent = msg.parent_file();
    let pkg = parent.package_name();
    let full = msg.full_name();
    let pkg_mod = pkg.replace('.', "_");

    let relative = if pkg.is_empty() {
        full
    } else {
        &full[pkg.len() + 1..]
    };

    let parts: Vec<&str> = relative.split('.').collect();
    let mut rust_path = pkg_mod;

    for (i, part) in parts.iter().enumerate() {
        rust_path.push_str("::");
        if i < parts.len() - 1 {
            rust_path.push_str(&to_snake_case(part));
        } else {
            rust_path.push_str(part);
        }
    }

    rust_path
}
