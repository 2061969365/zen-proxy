use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, StatusCode},
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

    let specified_port = env::var("PORT").ok().and_then(|p| p.parse::<u16>().ok());

    let (listener, actual_port) = match specified_port {
        Some(port) => {
            let bind_addr = format!("{}:{}", host, port);
            match tokio::net::TcpListener::bind(&bind_addr).await {
                Ok(l) => (l, port),
                Err(e) => {
                    eprintln!("❌ 端口绑定失败 ({}): {}", bind_addr, e);
                    wait_for_keypress();
                    return;
                }
            }
        }
        None => {
            let mut port = DEFAULT_PORT;
            let mut bound_listener = None;
            while port <= DEFAULT_PORT + 20 {
                let bind_addr = format!("{}:{}", host, port);
                match tokio::net::TcpListener::bind(&bind_addr).await {
                    Ok(l) => {
                        bound_listener = Some((l, port));
                        break;
                    }
                    Err(_) => {
                        port += 1;
                    }
                }
            }
            match bound_listener {
                Some(res) => res,
                None => {
                    eprintln!("❌ 无法在 4096~4116 找到可用端口（均被占用）");
                    wait_for_keypress();
                    return;
                }
            }
        }
    };

    let client = reqwest::Client::builder()
        .tcp_nodelay(true)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let state = Arc::new(AppState {
        client,
        zen_base: zen_base.clone(),
        user_agent,
        port: actual_port,
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

    println!("\n=======================================================");
    println!("  🚀 Zen Proxy 服务已启动！");
    println!("  - 本地地址: http://127.0.0.1:{}", actual_port);
    println!("  - 接口 Base URL: http://127.0.0.1:{}/v1", actual_port);
    println!("  - 状态检查: http://127.0.0.1:{}/api/status", actual_port);
    println!("  - 上游服务: {}", zen_base);
    println!("=======================================================");
    println!("  [运行中] 按 Ctrl+C 可停止服务...\n");

    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("❌ 运行错误: {}", e);
        wait_for_keypress();
    }
}

fn wait_for_keypress() {
    println!("\n按回车键退出程序...");
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
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

    req = attach_headers(req, &headers);

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
    let model = map_and_clean_model(&mut payload);
    let is_muse = model.starts_with("muse-spark");

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

    req = attach_headers(req, &headers);

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
    map_and_clean_model(&mut payload);

    let mut req = state
        .client
        .post(format!("{}/responses", state.zen_base))
        .header("User-Agent", &state.user_agent)
        .header("Content-Type", "application/json")
        .json(&payload);

    req = attach_headers(req, &headers);

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
) -> Response {
    let model = map_and_clean_model(&mut payload);
    let is_claude = model.starts_with("claude-");

    if is_claude {
        let mut req = state
            .client
            .post(format!("{}/messages", state.zen_base))
            .header("User-Agent", &state.user_agent)
            .header("Content-Type", "application/json")
            .json(&payload);

        req = attach_headers(req, &headers);

        return match req.send().await {
            Ok(res) => forward_response(res),
            Err(e) => (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": { "message": format!("Upstream request failed: {}", e) } })),
            )
                .into_response(),
        };
    }

    handle_anthropic_to_openai_adapter(state, headers, payload, model).await
}

async fn handle_anthropic_to_openai_adapter(
    state: Arc<AppState>,
    headers: HeaderMap,
    payload: Value,
    model: String,
) -> Response {
    let is_stream = payload
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);

    let mut openai_messages = Vec::new();

    if let Some(sys) = payload.get("system") {
        if let Some(s) = sys.as_str() {
            openai_messages.push(json!({ "role": "system", "content": s }));
        } else if let Some(arr) = sys.as_array() {
            let sys_text = arr
                .iter()
                .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n");
            if !sys_text.is_empty() {
                openai_messages.push(json!({ "role": "system", "content": sys_text }));
            }
        }
    }

    if let Some(messages) = payload.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content_val = msg.get("content");
            let text = if let Some(s) = content_val.and_then(|c| c.as_str()) {
                s.to_string()
            } else if let Some(arr) = content_val.and_then(|c| c.as_array()) {
                arr.iter()
                    .filter_map(|item| {
                        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                            Some(t.to_string())
                        } else if let Some(tr) = item.get("content").and_then(|c| c.as_str()) {
                            Some(tr.to_string())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                String::new()
            };

            openai_messages.push(json!({ "role": role, "content": text }));
        }
    }

    if model.starts_with("muse-spark") {
        let input_text = openai_messages
            .iter()
            .map(|m| format!("{}: {}", m["role"].as_str().unwrap_or("user"), m["content"].as_str().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n");

        let responses_payload = json!({
            "model": model,
            "input": input_text,
            "stream": false
        });

        let mut req = state
            .client
            .post(format!("{}/responses", state.zen_base))
            .header("User-Agent", &state.user_agent)
            .header("Content-Type", "application/json")
            .json(&responses_payload);

        req = attach_headers(req, &headers);

        return match req.send().await {
            Ok(res) => {
                if let Ok(resp_json) = res.json::<Value>().await {
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

                    if is_stream {
                        let sse_events = format!(
                            "event: message_start\ndata: {}\n\nevent: content_block_start\ndata: {}\n\nevent: content_block_delta\ndata: {}\n\nevent: content_block_stop\ndata: {}\n\nevent: message_delta\ndata: {}\n\nevent: message_stop\ndata: {}\n\n",
                            json!({
                                "type": "message_start",
                                "message": {
                                    "id": "msg_zen_muse",
                                    "type": "message",
                                    "role": "assistant",
                                    "model": model,
                                    "content": [],
                                    "stop_reason": null,
                                    "stop_sequence": null,
                                    "usage": { "input_tokens": 10, "output_tokens": 1 }
                                }
                            }),
                            json!({
                                "type": "content_block_start",
                                "index": 0,
                                "content_block": { "type": "text", "text": "" }
                            }),
                            json!({
                                "type": "content_block_delta",
                                "index": 0,
                                "delta": { "type": "text_delta", "text": text_output }
                            }),
                            json!({
                                "type": "content_block_stop",
                                "index": 0
                            }),
                            json!({
                                "type": "message_delta",
                                "delta": { "stop_reason": "end_turn", "stop_sequence": null },
                                "usage": { "output_tokens": 50 }
                            }),
                            json!({
                                "type": "message_stop"
                            })
                        );

                        return Response::builder()
                            .status(StatusCode::OK)
                            .header(header::CONTENT_TYPE, "text/event-stream")
                            .header(header::CACHE_CONTROL, "no-cache")
                            .body(Body::from(sse_events))
                            .unwrap_or_default();
                    }

                    let anthropic_response = json!({
                        "id": "msg_zen_muse",
                        "type": "message",
                        "role": "assistant",
                        "model": model,
                        "content": [
                            { "type": "text", "text": text_output }
                        ],
                        "stop_reason": "end_turn",
                        "stop_sequence": null,
                        "usage": { "input_tokens": 10, "output_tokens": 50 }
                    });
                    Json(anthropic_response).into_response()
                } else {
                    (StatusCode::INTERNAL_SERVER_ERROR, "Failed to parse muse response").into_response()
                }
            }
            Err(e) => (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": { "message": format!("Upstream request failed: {}", e) } })),
            ).into_response(),
        };
    }

    let openai_payload = json!({
        "model": model,
        "messages": openai_messages,
        "stream": false
    });

    let mut req = state
        .client
        .post(format!("{}/chat/completions", state.zen_base))
        .header("User-Agent", &state.user_agent)
        .header("Content-Type", "application/json")
        .json(&openai_payload);

    req = attach_headers(req, &headers);

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

    match res.json::<Value>().await {
        Ok(resp_json) => {
            let id = resp_json.get("id").and_then(|i| i.as_str()).unwrap_or("msg_zen_adapted");
            let text = resp_json
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c0| c0.get("message"))
                .and_then(|m| m.get("content"))
                .and_then(|t| t.as_str())
                .unwrap_or("");

            let input_tokens = resp_json
                .get("usage")
                .and_then(|u| u.get("prompt_tokens"))
                .and_then(|t| t.as_i64())
                .unwrap_or(0);
            let output_tokens = resp_json
                .get("usage")
                .and_then(|u| u.get("completion_tokens"))
                .and_then(|t| t.as_i64())
                .unwrap_or(0);

            if is_stream {
                let sse_events = format!(
                    "event: message_start\ndata: {}\n\nevent: content_block_start\ndata: {}\n\nevent: content_block_delta\ndata: {}\n\nevent: content_block_stop\ndata: {}\n\nevent: message_delta\ndata: {}\n\nevent: message_stop\ndata: {}\n\n",
                    json!({
                        "type": "message_start",
                        "message": {
                            "id": format!("msg_{}", id),
                            "type": "message",
                            "role": "assistant",
                            "model": model,
                            "content": [],
                            "stop_reason": null,
                            "stop_sequence": null,
                            "usage": { "input_tokens": input_tokens, "output_tokens": 1 }
                        }
                    }),
                    json!({
                        "type": "content_block_start",
                        "index": 0,
                        "content_block": { "type": "text", "text": "" }
                    }),
                    json!({
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": { "type": "text_delta", "text": text }
                    }),
                    json!({
                        "type": "content_block_stop",
                        "index": 0
                    }),
                    json!({
                        "type": "message_delta",
                        "delta": { "stop_reason": "end_turn", "stop_sequence": null },
                        "usage": { "output_tokens": output_tokens }
                    }),
                    json!({
                        "type": "message_stop"
                    })
                );

                return Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .header(header::CACHE_CONTROL, "no-cache")
                    .body(Body::from(sse_events))
                    .unwrap_or_default();
            }

            let anthropic_response = json!({
                "id": format!("msg_{}", id),
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [
                    { "type": "text", "text": text }
                ],
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens
                }
            });

            Json(anthropic_response).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": { "message": format!("Failed to parse upstream response: {}", e) } })),
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

    req = attach_headers(req, &headers);

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
        return forward_response(res);
    }

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

fn map_and_clean_model(payload: &mut Value) -> String {
    if let Some(m) = payload.get("model").and_then(|x| x.as_str()) {
        let mut cleaned = m.replace("[1m]", "").replace("[128k]", "");
        if cleaned.starts_with("claude-3-7-sonnet") {
            cleaned = "claude-sonnet-4-6".to_string();
        } else if cleaned.starts_with("claude-3-5-sonnet") {
            cleaned = "claude-sonnet-4-5".to_string();
        } else if cleaned.starts_with("claude-3-5-haiku") {
            cleaned = "claude-haiku-4-5".to_string();
        } else if cleaned.starts_with("claude-3-opus") {
            cleaned = "claude-opus-4-6".to_string();
        }
        payload["model"] = Value::String(cleaned.clone());
        cleaned
    } else {
        String::new()
    }
}

fn attach_headers(
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
    if let Some(ver) = headers.get("anthropic-version") {
        req = req.header("anthropic-version", ver);
    }
    if let Some(beta) = headers.get("anthropic-beta") {
        req = req.header("anthropic-beta", beta);
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
