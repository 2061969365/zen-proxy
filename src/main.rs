use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{env, sync::Arc};
use tower_http::cors::{Any, CorsLayer};

const DEFAULT_ZEN_BASE: &str = "https://opencode.ai/zen/v1";
const DEFAULT_USER_AGENT: &str = "opencode/1.18.18";
const DEFAULT_PORT: u16 = 4096;
const DEFAULT_HOST: &str = "0.0.0.0";

#[derive(Clone)]
struct AppState {
    client: reqwest::Client,
    zen_base: String,
    user_agent: String,
    port: u16,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let zen_base = env::var("ZEN_BASE").unwrap_or_else(|_| DEFAULT_ZEN_BASE.to_string());
    let user_agent = env::var("ZEN_USER_AGENT").unwrap_or_else(|_| DEFAULT_USER_AGENT.to_string());
    let host = env::var("HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let client = reqwest::Client::builder()
        .tcp_nodelay(true)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let state = Arc::new(AppState {
        client,
        zen_base,
        user_agent,
        port,
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/", get(status_handler))
        .route("/health", get(status_handler))
        .route("/api/status", get(status_handler))
        .route("/v1/models", get(models_handler))
        .route("/v1/chat/completions", post(chat_completions_handler))
        .route("/v1/responses", post(responses_handler))
        .route("/v1/messages", post(messages_handler))
        .layer(cors)
        .with_state(state);

    let bind_addr = format!("{}:{}", host, port);
    let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind to {}: {}", bind_addr, e);
            return;
        }
    };

    println!("zen-proxy listening on http://{}", bind_addr);
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("Server error: {}", e);
    }
}

async fn status_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "mode": "zen-rust",
        "port": state.port,
        "zen_base": state.zen_base,
        "version": env!("CARGO_PKG_VERSION")
    }))
}

async fn models_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let mut req = state
        .client
        .get(format!("{}/models", state.zen_base))
        .header("User-Agent", &state.user_agent);

    req = attach_auth_headers(req, &headers);

    match req.send().await {
        Ok(res) => forward_response(res),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": { "message": format!("Upstream request failed: {}", e) } })),
        )
            .into_response(),
    }
}

async fn chat_completions_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut payload): Json<Value>,
) -> impl IntoResponse {
    // Clean model suffixes like [1m] or [128k]
    clean_model_name(&mut payload);

    let model_str = payload
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();

    let is_muse = model_str.starts_with("muse-spark");

    // If muse-spark model and request uses standard messages format
    if is_muse && payload.get("input").is_none() && payload.get("messages").is_some() {
        return handle_muse_conversion(state, headers, payload).await;
    }

    let endpoint = if is_muse { "/responses" } else { "/chat/completions" };
    let mut req = state
        .client
        .post(format!("{}{}", state.zen_base, endpoint))
        .header("User-Agent", &state.user_agent)
        .header("Content-Type", "application/json")
        .json(&payload);

    req = attach_auth_headers(req, &headers);

    match req.send().await {
        Ok(res) => forward_response(res),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": { "message": format!("Upstream request failed: {}", e) } })),
        )
            .into_response(),
    }
}

async fn responses_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut payload): Json<Value>,
) -> impl IntoResponse {
    clean_model_name(&mut payload);

    let mut req = state
        .client
        .post(format!("{}/responses", state.zen_base))
        .header("User-Agent", &state.user_agent)
        .header("Content-Type", "application/json")
        .json(&payload);

    req = attach_auth_headers(req, &headers);

    match req.send().await {
        Ok(res) => forward_response(res),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": { "message": format!("Upstream request failed: {}", e) } })),
        )
            .into_response(),
    }
}

async fn messages_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut payload): Json<Value>,
) -> impl IntoResponse {
    clean_model_name(&mut payload);

    let mut req = state
        .client
        .post(format!("{}/messages", state.zen_base))
        .header("User-Agent", &state.user_agent)
        .header("Content-Type", "application/json")
        .json(&payload);

    req = attach_auth_headers(req, &headers);

    match req.send().await {
        Ok(res) => forward_response(res),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": { "message": format!("Upstream request failed: {}", e) } })),
        )
            .into_response(),
    }
}

async fn handle_muse_conversion(
    state: Arc<AppState>,
    headers: HeaderMap,
    payload: Value,
) -> Response {
    let model = payload
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("muse-spark-1.2-contributor-free")
        .to_string();

    let is_stream = payload
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);

    // Extract conversation text from messages array for responses API input
    let mut input_text = String::new();
    if let Some(messages) = payload.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = if let Some(text) = msg.get("content").and_then(|c| c.as_str()) {
                text.to_string()
            } else if let Some(arr) = msg.get("content").and_then(|c| c.as_array()) {
                arr.iter()
                    .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                String::new()
            };

            if messages.len() == 1 {
                input_text = content;
            } else {
                input_text.push_str(&format!("{}: {}\n", role, content));
            }
        }
    }

    let responses_payload = json!({
        "model": model,
        "input": input_text.trim(),
        "stream": is_stream
    });

    let mut req = state
        .client
        .post(format!("{}/responses", state.zen_base))
        .header("User-Agent", &state.user_agent)
        .header("Content-Type", "application/json")
        .json(&responses_payload);

    req = attach_auth_headers(req, &headers);

    let res = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": { "message": format!("Upstream request failed: {}", e) } })),
            )
                .into_response();
        }
    };

    if !res.status().is_success() {
        return forward_response(res);
    }

    if is_stream {
        // Stream direct response
        return forward_response(res);
    }

    // Convert /responses JSON output to standard chat.completion JSON
    match res.json::<Value>().await {
        Ok(resp_json) => {
            let id = resp_json
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("chatcmpl-zen-muse");
            let created = resp_json
                .get("created_at")
                .and_then(|c| c.as_i64())
                .unwrap_or(0);

            let mut text_output = String::new();
            if let Some(outputs) = resp_json.get("output").and_then(|o| o.as_array()) {
                for item in outputs {
                    if item.get("type").and_then(|t| t.as_str()) == Some("message") {
                        if let Some(contents) = item.get("content").and_then(|c| c.as_array()) {
                            for c in contents {
                                if let Some(text) = c.get("text").and_then(|t| t.as_str()) {
                                    text_output.push_str(text);
                                }
                            }
                        }
                    }
                }
            }

            let usage = resp_json.get("usage").cloned().unwrap_or_else(|| json!({}));

            let chat_completion = json!({
                "id": id,
                "object": "chat.completion",
                "created": created,
                "model": model,
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": text_output
                        },
                        "finish_reason": "stop"
                    }
                ],
                "usage": usage
            });

            Json(chat_completion).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": { "message": format!("Failed to parse response: {}", e) } })),
        )
            .into_response(),
    }
}

fn clean_model_name(payload: &mut Value) {
    if let Some(m) = payload.get("model").and_then(|x| x.as_str()) {
        let cleaned = m.replace("[1m]", "").replace("[128k]", "");
        payload["model"] = Value::String(cleaned);
    }
}

fn attach_auth_headers(
    mut req: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    if let Some(auth) = headers.get(header::AUTHORIZATION) {
        req = req.header(header::AUTHORIZATION, auth);
    }
    if let Some(api_key) = headers.get("x-api-key") {
        req = req.header("x-api-key", api_key);
    }
    if let Some(api_key) = headers.get("api-key") {
        req = req.header("api-key", api_key);
    }
    req
}

fn forward_response(res: reqwest::Response) -> Response {
    let status = res.status();
    let upstream_headers = res.headers().clone();
    let stream = res.bytes_stream();
    let body = Body::from_stream(stream);

    let mut response_builder = Response::builder().status(status);

    for (key, value) in upstream_headers.iter() {
        if key == header::CONTENT_TYPE
            || key == header::CACHE_CONTROL
            || key == header::CONTENT_ENCODING
        {
            response_builder = response_builder.header(key.as_str(), value.as_bytes());
        }
    }

    response_builder.body(body).unwrap_or_else(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": { "message": "Failed to create response body" } })),
        )
            .into_response()
    })
}

