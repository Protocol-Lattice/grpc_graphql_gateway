use grpc_graphql_gateway::gbp::GbpEncoder;
use serde_json::json;
use warp::{Filter, Reply};

#[tokio::main]
async fn main() {
    let users_route = warp::path("graphql")
        .and(warp::post())
        .and(warp::header::optional::<String>("accept"))
        .map(|accept: Option<String>| {
            let user_data = json!({
                "data": {
                    "_service": { "sdl": "extend type Query { me: User }" },
                    "me": {
                        "id": "1",
                        "username": "raezil",
                        "email": "raezil@protocol.lattice",
                        "role": "ADMIN",
                        "preferences": {
                            "theme": "dark",
                            "notifications": true
                        }
                    }
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
