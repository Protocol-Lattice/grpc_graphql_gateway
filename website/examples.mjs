export const operations = [
  {
    id: 'query', label: 'Queries', icon: 'search', filename: 'user.proto', language: 'protobuf',
    eyebrow: 'ASK FOR EXACTLY WHAT YOU NEED', title: 'Your RPC. A GraphQL query.',
    description: 'Annotate a protobuf method and let the gateway generate the schema. Your clients choose the fields. Your gRPC service does the work.',
    tags: ['Unary RPC', 'Generated schema', 'Typed responses'], docs: 'core/operations.html#queries',
    proto: `// Add to your annotated UserService\nrpc GetUser(GetUserRequest) returns (User) {\n  option (graphql.schema) = {\n    type: QUERY\n    name: "user"\n  };\n}`,
    graphql: `query GetUser {\n  user(id: "123") {\n    id\n    name\n    email\n  }\n}`,
    result: { data: { user: { id: '123', name: 'Alice', email: 'alice@example.com' } } },
    note: 'Illustrative response from the annotated GetUser service.',
  },
  {
    id: 'mutation', label: 'Mutations', icon: 'bolt', filename: 'user.proto', language: 'protobuf',
    eyebrow: 'MAKE SOMETHING HAPPEN', title: 'Write once. Change things.',
    description: 'Expose a write operation as a GraphQL mutation. Wrap the request fields in a typed input and route the call to your existing gRPC method.',
    tags: ['Typed inputs', 'Unary RPC', 'Existing services'], docs: 'core/operations.html#mutations',
    proto: `rpc CreateUser(CreateUserRequest) returns (User) {\n  option (graphql.schema) = {\n    type: MUTATION\n    name: "createUser"\n    request { name: "input" }\n  };\n}`,
    graphql: `mutation CreateUser {\n  createUser(input: {\n    name: "Alice"\n    email: "alice@example.com"\n  }) {\n    id\n    name\n  }\n}`,
    result: { data: { createUser: { id: '123', name: 'Alice' } } },
    note: 'Illustrative response. This demo does not create a user.',
  },
  {
    id: 'subscription', label: 'Subscriptions', icon: 'pulse', filename: 'user.proto', language: 'protobuf',
    eyebrow: 'KEEP THE CONVERSATION GOING', title: 'A stream of possibilities.',
    description: 'Turn a server-streaming RPC into a GraphQL subscription. Deliver ongoing updates over WebSocket using the graphql-transport-ws protocol.',
    tags: ['Server streaming', 'WebSocket', 'Real-time updates'], docs: 'core/operations.html#subscriptions',
    proto: `rpc WatchUser(WatchUserRequest) returns (stream User) {\n  option (graphql.schema) = {\n    type: SUBSCRIPTION\n    name: "userUpdates"\n  };\n}`,
    graphql: `subscription WatchUser {\n  userUpdates(id: "123") {\n    id\n    name\n    status\n  }\n}`,
    result: { data: { userUpdates: { id: '123', name: 'Alice', status: 'ONLINE' } } },
    note: 'Illustrative event. Connect your client to /graphql/ws to stream.',
  },
  {
    id: 'federation', label: 'Federation', icon: 'network', filename: 'user.proto', language: 'protobuf',
    eyebrow: 'A PLACE IN THE BIGGER PICTURE', title: 'Your service. The supergraph.',
    description: 'Define entity keys in protobuf and enable Federation v2. Compose your gRPC services with other subgraphs through Apollo Router.',
    tags: ['Federation v2', 'Entity keys', 'Apollo Router'], docs: 'federation/overview.html',
    proto: `message User {\n  option (graphql.entity) = {\n    keys: "id"\n    resolvable: true\n  };\n  string id = 1 [(graphql.field) = { required: true }];\n  string name = 2;\n  string email = 3;\n}`,
    graphql: `type User @key(fields: "id") {\n  id: ID!\n  name: String\n  email: String\n}`,
    result: { data: { _service: { sdl: 'type User @key(fields: "id") { id: ID! name: String email: String }' } } },
    note: 'Illustrative _service response after federation is enabled.',
  },
];

export const quickstart = [
  {
    id: 'rust', label: 'main.rs', language: 'rust',
    code: `use grpc_graphql_gateway::{Gateway, GrpcClient};\n\n#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {\n    Gateway::builder()\n        .with_descriptor_set_file("descriptor.bin")?\n        .add_grpc_client(\n            "greeter.Greeter",\n            GrpcClient::builder("http://localhost:50051")\n                .connect_lazy()?,\n        )\n        .build()?\n        .serve("0.0.0.0:8888")\n        .await?;\n\n    Ok(())\n}`,
  },
  { id: 'cargo', label: 'Cargo.toml', language: 'toml', code: `[dependencies]\ngrpc_graphql_gateway = "{{version}}"\ntokio = { version = "1", features = ["full"] }` },
  { id: 'proto', label: 'greeter.proto', language: 'protobuf', code: `syntax = "proto3";\npackage greeter;\n\nimport "graphql.proto";\n\nservice Greeter {\n  rpc SayHello(HelloRequest) returns (HelloReply) {\n    option (graphql.schema) = {\n      type: QUERY\n      name: "sayHello"\n    };\n  }\n}\n\nmessage HelloRequest { string name = 1; }\nmessage HelloReply { string message = 1; }` },
];
