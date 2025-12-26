use grpc_graphql_gateway::gbp::GbpEncoder;
use serde_json::json;
use warp::{Filter, Reply};

#[tokio::main]
async fn main() {
    let users_route = warp::path("graphql")
        .and(warp::post())
        .and(warp::header::optional::<String>("accept"))
        .map(|accept: Option<String>| {
            // Generate 20k users mocking real-time GraphQL dataset
            let users: Vec<_> = (0..10)
                .map(|i| {
                    json!({
                        "id": i,  // Only unique field
                        "typename": "User",
                        "status": "ACTIVE",
                        "role": "MEMBER",
                        "organization": {
                            "id": "org-protocol-lattice",
                            "name": "Protocol Lattice",
                            "settings": {
                                "theme": "dark",
                                "notifications": true,
                                "audit": "enabled",
                                "sso": true
                            }
                        },
                        "permissions": ["READ", "WRITE", "EXECUTE", "ADMIN", "OWNER"],
                        "profile": {
                            "verified": true,
                            "tier": "PREMIUM",
                            "avatar": "https://cdn.example.com/avatars/default.png",
                            "preferences": {
                                "language": "en",
                                "timezone": "UTC",
                                "newsletter": true
                            }
                        },
                        "metadata": {
                            "created_at": "2024-01-01T00:00:00Z",
                            "last_login": "2024-12-20T00:00:00Z",
                            "login_count": 500,
                            "tags": ["premium", "early-adopter", "verified"]
                        },
                        "description": "High-performance software engineer at Protocol Lattice working on distributed systems and GraphQL optimizations."
                    })
                })
                .collect();

            let user_data = json!({
                "data": {
                    "_service": { "sdl": "extend type Query { users: [User] }" },
                    "users": users
                }
            });
            handle_request(accept, user_data)
        });

    println!("🎭 Users Subgraph listening on http://0.0.0.0:4002/graphql");
    warp::serve(users_route).run(([0, 0, 0, 0], 4002)).await;
}

fn handle_request(accept: Option<String>, data: serde_json::Value) -> warp::reply::Response {
    if let Some(accept) = accept {
        if accept.contains("application/x-gbp") {
            let mut encoder = GbpEncoder::new();
            if let Ok(compressed) = encoder.encode_lz4(&data) {
                // Return GBP
                return warp::reply::with_header(compressed, "content-type", "application/x-gbp")
                    .into_response();
            }
        }
    }
    // Return JSON
    warp::reply::json(&data).into_response()
}
