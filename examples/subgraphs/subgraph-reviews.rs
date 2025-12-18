use grpc_graphql_gateway::gbp::GbpEncoder;
use serde_json::json;
use warp::{Filter, Reply};

#[tokio::main]
async fn main() {
    let reviews_route = warp::path("graphql")
        .and(warp::post())
        .and(warp::header::optional::<String>("accept"))
        .map(|accept: Option<String>| {
            // Generate some reviews
            let reviews: Vec<_> = (0..50)
                .map(|i| {
                    json!({
                        "id": format!("rev_{}", i),
                        "body": format!("Great product! Review #{}", i),
                        "rating": (i % 5) + 1,
                        "author": { "id": format!("user_{}", i % 10) },
                        "product": { "upc": format!("prod_{}", i % 5) }
                    })
                })
                .collect();

            let review_data = json!({
                "data": {
                    "_service": { "sdl": "extend type Query { reviews: [Review] }" },
                    "reviews": reviews
                }
            });
            handle_request(accept, review_data)
        });

    println!("⭐ Reviews Subgraph listening on http://0.0.0.0:4004/graphql");
    warp::serve(reviews_route).run(([0, 0, 0, 0], 4004)).await;
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
