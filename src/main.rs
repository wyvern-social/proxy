use axum::{
    extract::Query,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use base64::Engine;
use futures_util::TryStreamExt;
use hyper::Body;
use reqwest::Client;
use std::net::SocketAddr;
use tower_http::trace::TraceLayer;
use tracing_subscriber;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let app = Router::new()
        .route("/proxy", get(proxy_handler))
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("Media proxy running on http://{}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

#[derive(Debug, serde::Deserialize)]
struct ProxyParams {
    url: String,
}

async fn proxy_handler(Query(params): Query<ProxyParams>) -> impl IntoResponse {
    let decoded_url = match base64::engine::general_purpose::STANDARD.decode(&params.url) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid UTF-8 in URL").into_response(),
        },
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid base64 in URL").into_response(),
    };

    let url = match reqwest::Url::parse(&decoded_url) {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid URL").into_response(),
    };

    // Optional SSRF protection
    if url.domain().is_none() {
        return (StatusCode::BAD_REQUEST, "Missing or invalid domain").into_response();
    }

    if !["http", "https"].contains(&url.scheme()) {
        return (StatusCode::BAD_REQUEST, "Only http/https allowed").into_response();
    }

    let client = Client::new();

    let resp = match client.get(url.clone()).send().await {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_GATEWAY, "Failed to fetch upstream").into_response(),
    };

    if !resp.status().is_success() {
        return (StatusCode::BAD_GATEWAY, "Upstream returned error").into_response();
    }

    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string(); 

    if content_type.starts_with("text/html") {
        return (StatusCode::FORBIDDEN, "This is a media-based proxy only!").into_response();
    }

    let stream = resp
        .bytes_stream()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
    let body = Body::wrap_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=31536000")
        .body(axum::body::boxed(body))
        .unwrap()
}
