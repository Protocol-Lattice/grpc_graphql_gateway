use grpc_graphql_gateway::gbp::GbpEncoder;
use serde_json::json;
use warp::{Filter, Reply};

#[tokio::main]
async fn main() {
    let products_route = warp::path("graphql")
        .and(warp::post())
        .and(warp::header::optional::<String>("accept"))
        .map(|accept: Option<String>| {
            // Generate 20k products mocking real-time GraphQL dataset
            // Matches production patterns: user profiles, org data, permissions, nested metadata
            let products: Vec<_> = (0..20000)
                .map(|i| {
                    json!({
                        "id": i,  // Only unique field (integer for realistic DB IDs)
                        "typename": "Product",
                        "status": "ACTIVE",
                        "visibility": "PUBLIC",
                        "category": {
                            "id": "cat-electronics",
                            "name": "Electronics",
                            "slug": "electronics"
                        },
                        "organization": {
                            "id": "org-protocol-lattice",
                            "name": "Protocol Lattice",
                            "settings": {
                                "theme": "dark",
                                "notifications": true,
                                "audit": "enabled",
                                "region": "GLOBAL"
                            }
                        },
                        "permissions": ["READ", "WRITE", "EXECUTE", "ADMIN", "OWNER"],
                        "metadata": {
                            "verified": true,
                            "tier": "PREMIUM",
                            "tags": ["featured", "bestseller", "promoted"],
                            "analytics": {
                                "views": 10000,
                                "conversions": 500,
                                "rating": 4.8
                            }
                        },
                        "shipping": {
                            "eligible": true,
                            "methods": ["standard", "express", "same-day"],
                            "warehouse": "WH-CENTRAL-01"
                        },
                        "description": "High-performance enterprise-grade product with exceptional quality, durability, and comprehensive warranty coverage."
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
