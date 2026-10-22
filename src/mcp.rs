use crate::{App, Error, Result, modules::Call, oauth};
use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use rusqlite::params;
use serde_json::{Value, json};

const VERSION: &str = "2025-11-25";
fn rpc_error(id: Value, code: i64, message: &str) -> Response {
    (
        StatusCode::OK,
        Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})),
    )
        .into_response()
}
pub async fn handle(State(app): State<App>, headers: HeaderMap, body: Bytes) -> Result<Response> {
    let actor = oauth::authenticate(&app, &headers)?;
    if !headers
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| {
            s.split(';')
                .next()
                .is_some_and(|m| m.trim() == "application/json")
        })
    {
        return Err(Error::new(
            415,
            "unsupported_media_type",
            "MCP requests require application/json",
        ));
    }
    if !headers
        .get("accept")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| s.contains("application/json") && s.contains("text/event-stream"))
    {
        return Err(Error::new(
            406,
            "not_acceptable",
            "Accept must include application/json and text/event-stream",
        ));
    }
    if headers.get("mcp-protocol-version").is_some_and(|v| {
        !matches!(
            v.to_str().unwrap_or(""),
            "2025-11-25" | "2025-06-18" | "2025-03-26"
        )
    }) {
        return Err(Error::invalid("Unsupported MCP protocol version"));
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Ok(rpc_error(Value::Null, -32700, "Parse error")),
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if !request.is_object()
        || request["jsonrpc"] != "2.0"
        || !request["method"].is_string()
        || (!id.is_null() && !id.is_string() && id.as_i64().is_none())
        || request.get("params").is_some_and(|p| !p.is_object())
    {
        return Ok(rpc_error(id, -32600, "Invalid request"));
    }
    let method = request["method"].as_str().unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(json!({}));
    if request.get("id").is_none() {
        return if method.starts_with("notifications/") {
            Ok(StatusCode::ACCEPTED.into_response())
        } else {
            Ok(rpc_error(Value::Null, -32600, "Requests require an ID"))
        };
    }
    let result = match method {
        "initialize" => {
            let requested = params["protocolVersion"].as_str().unwrap_or(VERSION);
            let version = if matches!(requested, "2025-11-25" | "2025-06-18" | "2025-03-26") {
                requested
            } else {
                VERSION
            };
            Ok(
                json!({"protocolVersion":version,"serverInfo":{"name":"ensemble-mcp","title":"Team Ensemble","version":env!("CARGO_PKG_VERSION")},"capabilities":{"tools":{},"resources":{}},"instructions":"Use service tools with this connection's consent. Reconnect in the organisation portal if a service requires consent. Never retry an uncertain mutation without checking its outcome."}),
            )
        }
        "ping" => Ok(json!({})),
        "tools/list" => {
            let tools = app
                .modules
                .modules
                .iter()
                .flat_map(|m| {
                    m.tools
                        .iter()
                        .filter(|t| oauth::allowed(&app, &actor, m.id, t.write).is_ok())
                        .map(|t| t.metadata(m.id))
                })
                .collect::<Vec<_>>();
            Ok(json!({"tools":tools}))
        }
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return Ok(rpc_error(id, -32602, "Tool name is required"));
            };
            let (module, tool) = match app.modules.tool(name) {
                Ok(t) => t,
                Err(_) => return Ok(rpc_error(id, -32602, "Unknown tool")),
            };
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if let Err(e) = tool.validate(&args) {
                return Ok(rpc_error(id, -32602, e.message));
            }
            let req = crate::web::request_id(&headers);
            let mutation_context = std::sync::Arc::new(std::sync::Mutex::new(None));
            let executed = tokio::time::timeout(std::time::Duration::from_secs(90), async {
                oauth::allowed(&app, &actor, module.id, tool.write)?;
                let token = app
                    .provider
                    .access_token(&app, &actor.user, module.id)
                    .await?;
                oauth::allowed(&app, &actor, module.id, tool.write)?;
                (module.handler)(
                    Call {
                        app: app.clone(),
                        actor: actor.clone(),
                        token,
                        args,
                        mutation_context: mutation_context.clone(),
                    },
                    name.into(),
                )
                .await
            })
            .await
            .unwrap_or_else(|_| {
                let error = if tool.write {
                    Error::new(
                        504,
                        "mutation_uncertain",
                        "Operation deadline reached; check the outcome before repeating",
                    )
                } else {
                    Error::new(
                        504,
                        "provider_unavailable",
                        "Operation deadline reached; try a smaller request",
                    )
                };
                Err(error)
            })
            .map_err(|mut error| {
                if let Ok(saved) = mutation_context.lock()
                    && let Some(Value::Object(mut context)) = saved.clone()
                {
                    context.insert("cause".into(), json!(error.code));
                    if let Some(details) = error.context.take() {
                        if let Value::Object(details) = details {
                            context.extend(details);
                        } else {
                            context.insert("details".into(), details);
                        }
                    }
                    error.context = Some(Value::Object(context));
                }
                error
            });
            app.store.audit(
                req,
                Some(actor.user.id),
                Some(&actor.client),
                name,
                &match &executed {
                    Ok(_) => "ok".into(),
                    Err(e) => e
                        .provider_detail()
                        .map_or_else(|| e.code.to_owned(), |d| format!("{} ({d})", e.code)),
                },
            )?;
            app.store.with(|db| {
                db.execute(
                    "UPDATE connections SET health=?,checked=? WHERE user_id=? AND provider=?",
                    params![
                        executed
                            .as_ref()
                            .map(|_| "connected")
                            .unwrap_or_else(|e| e.code),
                        crate::store::now(),
                        actor.user.id,
                        module.provider
                    ],
                )?;
                Ok(())
            })?;
            match executed {
                Ok(mut value) => {
                    let mut content = value
                        .as_object_mut()
                        .and_then(|o| o.remove("content"))
                        .and_then(|v| v.as_array().cloned())
                        .unwrap_or_default();
                    let text = serde_json::to_string(&value)?;
                    content.insert(0,json!({"type":"text","text":if text.len()>12000 {format!("{} operation completed. Full data is in structuredContent.",module.name)}else{text}}));
                    Ok(json!({"content":content,"structuredContent":value,"isError":false}))
                }
                Err(e) => {
                    if matches!(e.status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
                        return Err(e);
                    }
                    let mut text = format!("{}: {}", e.code, e.message);
                    let mut meta = json!({"error_code":e.code});
                    if let Some(context) = e.context {
                        text.push_str(&format!("\n{}", serde_json::to_string(&context)?));
                        meta["error_context"] = context;
                    }
                    Ok(json!({"isError":true,"content":[{"type":"text","text":text}],"_meta":meta}))
                }
            }
        }
        "resources/list" | "resources/templates/list" | "resources/read" => tokio::time::timeout(
            std::time::Duration::from_secs(60),
            resources(&app, &actor, method, params),
        )
        .await
        .unwrap_or_else(|_| {
            Err(Error::new(
                504,
                "provider_unavailable",
                "Resource deadline reached; use direct reads or a smaller list",
            ))
        }),
        _ => return Ok(rpc_error(id, -32601, "Method not found")),
    };
    match result {
        Ok(value) => Ok(Json(json!({"jsonrpc":"2.0","id":id,"result":value})).into_response()),
        Err(e) => {
            app.store.audit(
                crate::web::request_id(&headers),
                Some(actor.user.id),
                Some(&actor.client),
                "resource.operation",
                e.code,
            )?;
            if matches!(e.status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
                return Err(e);
            }
            Ok(rpc_error(
                id,
                if e.code == "invalid_request" {
                    -32602
                } else {
                    -32000
                },
                &format!("{}: {}", e.code, e.message),
            ))
        }
    }
}

async fn resources(
    app: &App,
    actor: &oauth::Actor,
    method: &str,
    mut params: Value,
) -> Result<Value> {
    if method == "resources/read" {
        let uri = params["uri"]
            .as_str()
            .ok_or_else(|| Error::invalid("Resource URI is required"))?;
        if uri.starts_with("ui://") {
            for module in &app.modules.modules {
                for tool in &module.tools {
                    if let Some(ui) = &tool.ui
                        && ui.uri == uri
                    {
                        oauth::allowed(app, actor, module.id, tool.write)?;
                        return Ok(ui.read());
                    }
                }
            }
            return Err(Error::invalid("Unknown app resource URI"));
        }
        let module = app
            .modules
            .modules
            .iter()
            .find(|m| {
                m.resources
                    .as_ref()
                    .is_some_and(|r| uri.starts_with(&format!("{}://", r.scheme)))
            })
            .ok_or_else(|| Error::invalid("Unknown resource URI"))?;
        oauth::allowed(app, actor, module.id, false)?;
        return (module
            .resources
            .as_ref()
            .ok_or_else(Error::internal)?
            .handler)(
            Call {
                app: app.clone(),
                actor: actor.clone(),
                token: String::new(),
                args: params,
                mutation_context: Default::default(),
            },
            method.into(),
        )
        .await;
    }
    let available = app
        .modules
        .modules
        .iter()
        .filter(|m| m.resources.is_some() && oauth::allowed(app, actor, m.id, false).is_ok())
        .collect::<Vec<_>>();
    if method == "resources/templates/list" {
        let mut templates = vec![];
        for module in available {
            let out = (module
                .resources
                .as_ref()
                .ok_or_else(Error::internal)?
                .handler)(
                Call {
                    app: app.clone(),
                    actor: actor.clone(),
                    token: String::new(),
                    args: json!({}),
                    mutation_context: Default::default(),
                },
                method.into(),
            )
            .await?;
            templates.extend(
                out["resourceTemplates"]
                    .as_array()
                    .cloned()
                    .ok_or_else(Error::internal)?,
            );
        }
        return Ok(json!({"resourceTemplates":templates}));
    }
    let cursor = params["cursor"].as_str().map(String::from);
    let (index, upstream) = if let Some(cursor) = cursor.as_deref() {
        let (module, upstream) = cursor
            .split_once(':')
            .ok_or_else(|| Error::invalid("Invalid resource cursor"))?;
        (
            available
                .iter()
                .position(|m| m.id == module)
                .ok_or_else(Error::denied)?,
            Some(upstream.to_owned()),
        )
    } else {
        (0, None)
    };
    let mut items = vec![];
    if cursor.is_none() {
        for module in &app.modules.modules {
            for tool in &module.tools {
                if let Some(ui) = &tool.ui
                    && oauth::allowed(app, actor, module.id, tool.write).is_ok()
                {
                    items.push(ui.metadata());
                }
            }
        }
    }
    let mut out = json!({});
    if let Some(module) = available.get(index) {
        params["cursor"] = upstream.map(Value::String).unwrap_or(Value::Null);
        let result = (module
            .resources
            .as_ref()
            .ok_or_else(Error::internal)?
            .handler)(
            Call {
                app: app.clone(),
                actor: actor.clone(),
                token: String::new(),
                args: params,
                mutation_context: Default::default(),
            },
            method.into(),
        )
        .await?;
        items.extend(
            result["resources"]
                .as_array()
                .cloned()
                .ok_or_else(Error::internal)?,
        );
        if let Some(next) = result["nextCursor"].as_str() {
            out["nextCursor"] = json!(format!("{}:{next}", module.id));
        } else if let Some(next) = available.get(index + 1) {
            out["nextCursor"] = json!(format!("{}:0", next.id));
        }
    }
    out["resources"] = json!(items);
    Ok(out)
}
