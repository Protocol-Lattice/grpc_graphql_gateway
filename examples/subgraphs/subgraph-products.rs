use grpc_graphql_gateway::gbp::GbpEncoder;
use serde_json::json;
use warp::{Filter, Reply};

#[tokio::main]
async fn main() {
    let products_route = warp::path("graphql")
        .and(warp::post())
        .and(warp::header::optional::<String>("accept"))
        .map(|accept: Option<String>| {
            // Generate some products
            let products: Vec<_> = (0..10000)
                .map(|i| {
                    json!({
                        "id": format!("prod_{}", i),
                        "name": format!("High Performance Widget v{}", i),
                        "price": 99.99 + (i as f64),
                        "stock": 1000 - i,
                        "tags": ["performance", "rust", "gbp"]
                    })
                })
                .collect();

            let product_data = json!({
                "data": {
                    "_service": { "sdl": "extend type Query { products: [Product] }" },
                    "products": products
                }
            });
            handle_request(accept, product_data)
        });

    println!("📦 Products Subgraph listening on http://0.0.0.0:4003/graphql");
    warp::serve(products_route).run(([0, 0, 0, 0], 4003)).await;
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
