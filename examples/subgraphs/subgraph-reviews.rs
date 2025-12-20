use grpc_graphql_gateway::gbp::GbpEncoder;
use serde_json::json;
use warp::{Filter, Reply};

#[tokio::main]
async fn main() {
    let reviews_route = warp::path("graphql")
        .and(warp::post())
        .and(warp::header::optional::<String>("accept"))
        .map(|accept: Option<String>| {
            // Generate 20k reviews mocking real-time GraphQL dataset
            let reviews: Vec<_> = (0..20000)
                .map(|i| {
                    json!({
                        "id": i,  // Only unique field
                        "typename": "Review",
                        "status": "APPROVED",
                        "visibility": "PUBLIC",
                        "rating": 5,
                        "author": {
                            "id": "author-verified",
                            "displayName": "Verified Buyer",
                            "avatar": "https://cdn.example.com/avatars/default.png",
                            "badges": ["verified", "top-contributor", "early-adopter"]
                        },
                        "product": {
                            "id": "prod-featured",
                            "name": "Featured Product"
                        },
                        "moderation": {
                            "reviewed": true,
                            "flagged": false,
                            "moderator": "system-auto"
                        },
                        "analytics": {
                            "helpful_votes": 150,
                            "not_helpful_votes": 3,
                            "reports": 0
                        },
                        "content": {
                            "title": "Excellent product, highly recommended!",
                            "body": "This product exceeded all my expectations. The quality is outstanding and the customer service was exceptional.",
                            "pros": ["high quality", "fast shipping", "great value"],
                            "cons": ["none"]
                        }
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
