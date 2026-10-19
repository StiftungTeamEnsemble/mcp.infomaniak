//! Provider adapter: https://github.com/Infomaniak/mcp-server-kdrive/blob/main/src/kdrive-client.ts
use super::*;
use crate::identity::{Provider, binary};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::Method;
use serde_json::json;

pub fn module() -> Module {
    let mut tools = vec![
        tool(
            "kdrive_search",
            "Search an authorised drive or folder; returns bounded matches and continuation.",
            false,
            false,
            true,
            &["query"],
            vec![
                ("query", id()),
                ("directory_id", n()),
                ("depth", choices(&["child", "unlimited"])),
            ],
        ),
        tool(
            "kdrive_list",
            "List folder items with provider continuation.",
            false,
            false,
            true,
            &[],
            vec![
                ("file_id", n()),
                ("type", choices(&["file", "dir"])),
                ("cursor", id()),
            ],
        ),
        tool(
            "kdrive_info",
            "Read item metadata including drive identity and source navigation.",
            false,
            false,
            true,
            &["file_id"],
            vec![("file_id", n())],
        ),
        tool(
            "kdrive_recents",
            "List recently accessed items.",
            false,
            false,
            true,
            &[],
            vec![("type", choices(&["file", "dir", "vault"]))],
        ),
        tool(
            "kdrive_shared_with_me",
            "List items explicitly shared with the acting user.",
            false,
            false,
            true,
            &[],
            vec![("type", choices(&["file", "dir", "vault"]))],
        ),
        tool(
            "kdrive_upload_file",
            "Upload exactly one UTF-8 text or base64 content value, up to 8 MiB.",
            true,
            false,
            true,
            &["file_name"],
            vec![
                ("file_name", id()),
                ("text_content", json!({"type":"string","maxLength":8388608})),
                (
                    "base64_content",
                    json!({"type":"string","maxLength":11184812}),
                ),
                ("directory_id", n()),
            ],
        ),
        tool(
            "kdrive_download_file",
            "Download a user-authorised file as typed MCP content.",
            false,
            false,
            true,
            &["file_id"],
            vec![("file_id", n())],
        ),
        tool(
            "kdrive_create_folder",
            "Create a directory under file_id (default root 1).",
            true,
            false,
            true,
            &["name"],
            vec![("name", id()), ("file_id", n())],
        ),
        tool(
            "kdrive_rename",
            "Rename a file or directory.",
            true,
            true,
            true,
            &["file_id", "name"],
            vec![("file_id", n()), ("name", id())],
        ),
        tool(
            "kdrive_move",
            "Move an item to a destination directory.",
            true,
            true,
            true,
            &["file_id", "dest_id"],
            vec![("file_id", n()), ("dest_id", n())],
        ),
        tool(
            "kdrive_delete",
            "Move an item to Trash.",
            true,
            true,
            true,
            &["file_id"],
            vec![("file_id", n())],
        ),
        tool(
            "kdrive_restore",
            "Restore an item to an explicit destination directory.",
            true,
            false,
            true,
            &["file_id", "destination_directory_id"],
            vec![("file_id", n()), ("destination_directory_id", n())],
        ),
        tool(
            "kdrive_share_link",
            "Create a sharing link within the acting user's delegation rights.",
            true,
            true,
            true,
            &["file_id", "right"],
            vec![
                ("file_id", n()),
                ("right", choices(&["inherit", "password", "public"])),
                ("password", id()),
                ("valid_until", n()),
                ("can_download", b()),
                ("can_edit", b()),
                ("can_request_access", b()),
            ],
        ),
        tool(
            "kdrive_share_access",
            "Invite recipients with explicitly selected access rights.",
            true,
            true,
            true,
            &["file_id", "emails", "right"],
            vec![
                ("file_id", n()),
                ("emails", arr(id())),
                ("right", choices(&["manage", "read", "write"])),
                ("message", s()),
            ],
        ),
    ];
    for t in &mut tools {
        let item = json!({"type":"object","properties":{"id":{"type":"integer"},"name":{"type":"string"},"type":{"type":"string"},"size":{"type":["integer","null"]},"path":{"type":["string","null"]},"mime_type":{"type":["string","null"]},"last_modified_at":{"type":["string","integer","null"]},"url":{"type":["string","null"]}}});
        t.output = match t.name.as_str() {
            "kdrive_list" | "kdrive_search" | "kdrive_recents" | "kdrive_shared_with_me" => {
                json!({"type":"array","items":item})
            }
            "kdrive_download_file" => {
                json!({"type":"object","properties":{"filename":{"type":"string"},"mime_type":{"type":"string"},"size":{"type":"integer"}},"required":["filename","mime_type","size"]})
            }
            _ => item,
        };
        if matches!(
            t.name.as_str(),
            "kdrive_list" | "kdrive_search" | "kdrive_recents" | "kdrive_shared_with_me"
        ) {
            t.schema["properties"]["cursor"] = json!({"type":"string","minLength":1,"maxLength":2048,"description":"Continuation cursor from the previous page; retain the same drive and filters"});
        }
        t.schema["properties"]["drive_id"] = n();
        t.schema["required"]
            .as_array_mut()
            .unwrap()
            .push(json!("drive_id"));
    }
    Module {
        id: "kdrive",
        name: "kDrive",
        namespace: "kdrive",
        scopes: vec!["drive", "user_info"],
        tools,
        routes: vec![],
        resources: Some(Resources {
            scheme: "kdrive",
            handler: |c, op| Box::pin(resource_call(c, op)),
        }),
        handler: |c, n| Box::pin(call(c, n)),
    }
}
pub async fn call(c: Call, name: String) -> Result<Value> {
    let args = &c.args;
    let drive = numeric(args, "drive_id", 0);
    let file = numeric(args, "file_id", 1);
    let op = name
        .strip_prefix("kdrive_")
        .ok_or_else(|| Error::invalid("Unknown kDrive operation"))?;
    let api = &c.app.provider.api;
    let v3 = format!("{api}/3/drive/{drive}");
    let v2 = format!("{api}/2/drive/{drive}");
    let mut query = vec![];
    let mut body = None;
    let (method, url) = match op {
        "search" => {
            query.push(("query".into(), text(args, "query")?.into()));
            for k in ["directory_id", "depth"] {
                if let Some(v) = args.get(k) {
                    query.push((
                        k.into(),
                        v.as_str()
                            .map(String::from)
                            .unwrap_or_else(|| v.to_string()),
                    ));
                }
            }
            (Method::GET, format!("{v3}/files/search"))
        }
        "list" => (Method::GET, format!("{v3}/files/{file}/files")),
        "info" => (Method::GET, format!("{v3}/files/{file}")),
        "recents" => (Method::GET, format!("{v3}/files/recents")),
        "shared_with_me" => (Method::GET, format!("{v3}/files/shared_with_me")),
        "create_folder" => {
            body = Some(json!({"name":args["name"]}));
            (Method::POST, format!("{v3}/files/{file}/directory"))
        }
        "rename" => {
            body = Some(json!({"name":args["name"]}));
            (Method::POST, format!("{v2}/files/{file}/rename"))
        }
        "move" => (
            Method::POST,
            format!("{v3}/files/{file}/move/{}", numeric(args, "dest_id", 0)),
        ),
        "delete" => (Method::DELETE, format!("{v2}/files/{file}")),
        "restore" => {
            body = Some(json!({"destination_directory_id":args["destination_directory_id"]}));
            (Method::POST, format!("{v2}/trash/{file}/restore"))
        }
        "share_link" => {
            if args["right"] == "password"
                && args["password"]
                    .as_str()
                    .is_none_or(|s| s.trim().is_empty())
            {
                return Err(Error::invalid("Password access requires a password"));
            }
            if args["valid_until"]
                .as_i64()
                .is_some_and(|t| t <= crate::store::now())
            {
                return Err(Error::invalid("Sharing expiry must be in the future"));
            }
            let mut options = args.clone();
            for key in ["drive_id", "file_id"] {
                options.as_object_mut().unwrap().remove(key);
            }
            body = Some(options);
            (Method::POST, format!("{v2}/files/{file}/link"))
        }
        "share_access" => {
            if args["emails"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| !email(v.as_str().unwrap_or("")))
            {
                return Err(Error::invalid("Invalid invitation email"));
            }
            let mut options = args.clone();
            for key in ["drive_id", "file_id"] {
                options.as_object_mut().unwrap().remove(key);
            }
            body = Some(options);
            (
                Method::POST,
                format!("{v2}/files/{file}/access/invitations"),
            )
        }
        "upload_file" => {
            if args.get("text_content").is_some() == args.get("base64_content").is_some() {
                return Err(Error::invalid(
                    "Supply exactly one of text_content and base64_content",
                ));
            }
            let bytes = if let Some(s) = args["text_content"].as_str() {
                s.as_bytes().to_vec()
            } else {
                STANDARD
                    .decode(text(args, "base64_content")?)
                    .map_err(|_| Error::invalid("Invalid base64 content"))?
            };
            if bytes.len() > crate::identity::MAX_BYTES {
                return Err(Error::new(
                    413,
                    "content_too_large",
                    "Content exceeds the 8 MiB limit",
                ));
            }
            let q = [
                ("directory_id", numeric(args, "directory_id", 1).to_string()),
                ("file_name", text(args, "file_name")?.into()),
                ("total_size", bytes.len().to_string()),
            ];
            c.set_mutation_context(json!({"drive_id":drive,"directory_id":numeric(args,"directory_id",1),"phase":op,"recovery":"Inspect the destination folder before repeating"}))?;
            let response = Provider::json(
                c.app
                    .provider
                    .http
                    .post(format!("{v3}/upload"))
                    .bearer_auth(&c.token)
                    .query(&q)
                    .header("content-type", "application/octet-stream")
                    .body(bytes),
                true,
            )
            .await?;
            let item = file_metadata(data(response), drive, None, true)?;
            remember_files(&c.app, c.actor.user.id, drive, &item)?;
            return Ok(result(
                item,
                json!({"drive_id":drive,"directory_id":numeric(args,"directory_id",1)}),
            ));
        }
        "download_file" => {
            let metadata = file_metadata(
                data(
                    c.app
                        .provider
                        .request(
                            &c.token,
                            Method::GET,
                            format!("{v3}/files/{file}"),
                            &[],
                            None,
                        )
                        .await?,
                ),
                drive,
                Some(file),
                false,
            )?;
            let (_, bytes) = Provider::bytes(
                c.app
                    .provider
                    .http
                    .get(format!("{v2}/files/{file}/download"))
                    .bearer_auth(&c.token),
                false,
            )
            .await?;
            let mut out = binary(
                format!("kdrive://{drive}/{file}"),
                metadata["name"].as_str().unwrap_or("file").into(),
                metadata["mime_type"]
                    .as_str()
                    .unwrap_or("application/octet-stream")
                    .into(),
                bytes,
            );
            let content = out.as_object_mut().unwrap().remove("content").unwrap();
            return Ok(
                json!({"data":out,"context":{"drive_id":drive,"file_id":file},"content":content}),
            );
        }
        _ => return Err(Error::invalid("Unknown kDrive operation")),
    };
    let listing = matches!(op, "list" | "search" | "recents" | "shared_with_me");
    if listing {
        query.push(("limit".into(), "50".into()));
    }
    if let Some(kind) = args["type"].as_str() {
        query.push(("type[]".into(), kind.into()));
    }
    if let Some(cursor) = args["cursor"].as_str() {
        query.push(("cursor".into(), cursor.into()));
    }
    if method != Method::GET {
        c.set_mutation_context(json!({"drive_id":drive,"file_id":file,"dest_id":args["dest_id"],"destination_directory_id":args["destination_directory_id"],"phase":op,"recovery":"Inspect the item, destination, Trash or sharing settings before repeating"}))?;
    }
    let response = c
        .app
        .provider
        .request(&c.token, method, url, &query, body)
        .await?;
    let has_more = response["has_more"].as_bool().unwrap_or(false);
    let cursor = if listing && has_more {
        let cursor = response["cursor"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 2048 && Some(*s) != args["cursor"].as_str())
            .ok_or_else(|| {
                Error::new(
                    502,
                    "provider_error",
                    "Provider returned incomplete results without a usable continuation cursor",
                )
            })?;
        json!(cursor)
    } else {
        Value::Null
    };
    let mut items = data(response);
    if listing && items.as_array().is_none_or(|items| items.len() > 50) {
        return Err(Error::new(
            502,
            "provider_error",
            "Invalid or oversized provider file page",
        ));
    }
    if let Some(page) = items.as_array_mut().filter(|_| listing) {
        for item in page {
            *item = file_metadata(std::mem::take(item), drive, None, false)?;
        }
    } else if op == "info" || op == "create_folder" {
        items = file_metadata(items, drive, (op == "info").then_some(file), op != "info")?;
    } else if matches!(op, "rename" | "move" | "restore" | "delete") {
        items = file_metadata(items, drive, Some(file), true)?;
    }
    if listing || matches!(op, "info" | "create_folder") {
        remember_files(&c.app, c.actor.user.id, drive, &items)?;
    }
    let mut out = result(
        items,
        json!({"drive_id":drive,"file_id":file,"source_url":format!("https://kdrive.infomaniak.com/app/drive/{drive}/files/{file}")}),
    );
    out["has_more"] = json!(has_more);
    out["cursor"] = cursor;
    Ok(out)
}

fn file_metadata(
    mut item: Value,
    drive: i64,
    expected: Option<i64>,
    mutation: bool,
) -> Result<Value> {
    let numeric_id = |value: &Value| {
        value
            .as_i64()
            .filter(|id| (1..=9007199254740991).contains(id))
    };
    let returned_id = numeric_id(&item["id"]);
    // A write acknowledgement may omit its ID, but any returned identity
    // must agree with the requested object. Creation always needs a new ID.
    let id = if mutation && item.get("id").is_none() {
        expected
    } else {
        returned_id
    };
    if !item.is_object()
        || id.is_none()
        || expected.is_some_and(|expected| id != Some(expected))
        || item
            .get("drive_id")
            .is_some_and(|id| id.as_i64() != Some(drive))
    {
        let mut error = Error::new(
            502,
            if mutation {
                "mutation_uncertain"
            } else {
                "provider_error"
            },
            "Provider returned an invalid file or drive identity; verify any mutation before repeating",
        );
        if mutation {
            error = error.with_context(json!({"returned_file_id":returned_id,"returned_drive_id":numeric_id(&item["drive_id"])}));
        }
        return Err(error);
    }
    item["id"] = json!(id);
    item["drive_id"] = json!(drive);
    item["source_url"] = json!(format!(
        "https://kdrive.infomaniak.com/app/drive/{drive}/files/{}",
        id.unwrap()
    ));
    Ok(item)
}

use crate::{App, oauth};
use rusqlite::params;
fn remember_files(app: &App, user: i64, drive: i64, value: &Value) -> Result<()> {
    let items = if let Some(items) = value.as_array() {
        items.clone()
    } else {
        vec![value.clone()]
    };
    app.store.with(|c|{
        for item in items {
            if item["type"]=="dir" || item["is_dir"]==true {continue}
            if let (Some(id),Some(name))=(item["id"].as_i64(),item["name"].as_str()) {
                let mime=item["mime_type"].as_str().unwrap_or("application/octet-stream");
                c.execute("INSERT INTO resource_refs(user_id,drive_id,file_id,name,mime) VALUES(?,?,?,?,?) ON CONFLICT(user_id,drive_id,file_id) DO UPDATE SET name=excluded.name,mime=excluded.mime",params![user,drive,id,name,mime])?;
            }
        }
        // Metadata discovery is a convenience, never an authorisation cache.
        c.execute("DELETE FROM resource_refs WHERE user_id=? AND rowid NOT IN (SELECT rowid FROM resource_refs WHERE user_id=? ORDER BY rowid DESC LIMIT 1000)",params![user,user])?; Ok(())
    })
}
async fn list_resources(app: &App, actor: &oauth::Actor, params: &Value) -> Result<Value> {
    let mut resources = vec![];
    if oauth::allowed(app, actor, "kdrive", false).is_err() {
        return Ok(json!({"resources":resources}));
    }
    let before = params["cursor"]
        .as_str()
        .map(|s| s.parse::<i64>())
        .transpose()
        .map_err(|_| Error::invalid("Invalid resource cursor"))?
        .unwrap_or(0);
    if before < 0 {
        return Err(Error::invalid("Invalid resource cursor"));
    }
    let mut refs=app.store.with(|c|{
        let mut stmt=c.prepare("SELECT rowid,drive_id,file_id FROM resource_refs WHERE user_id=? AND (?=0 OR rowid<?) ORDER BY rowid DESC LIMIT 21")?;
        Ok(stmt.query_map(params![actor.user.id,before,before],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
    })?;
    let more = refs.len() > 20;
    refs.truncate(20);
    let token = app
        .provider
        .access_token(app, &actor.user, "kdrive")
        .await?;
    oauth::allowed(app, actor, "kdrive", false)?;
    for (_, drive, file) in &refs {
        let item = app
            .provider
            .request(
                &token,
                reqwest::Method::GET,
                format!("{}/3/drive/{drive}/files/{file}", app.provider.api),
                &[],
                None,
            )
            .await;
        match item {
            Ok(item) => {
                let item = file_metadata(data(item), *drive, Some(*file), false)?;
                resources.push(json!({"uri":format!("kdrive://{drive}/{file}"),"name":item["name"].as_str().unwrap_or("file"),"mimeType":item["mime_type"].as_str().unwrap_or("application/octet-stream")}));
            }
            Err(e) if matches!(e.code, "access_denied" | "provider_not_found") => {
                app.store.with(|db| {
                    db.execute(
                        "DELETE FROM resource_refs WHERE user_id=? AND drive_id=? AND file_id=?",
                        params![actor.user.id, drive, file],
                    )?;
                    Ok(())
                })?;
            }
            Err(e) => return Err(e),
        }
    }
    let mut out = json!({"resources":resources});
    if more && let Some((last, _, _)) = refs.last() {
        out["nextCursor"] = json!(last.to_string());
    }
    Ok(out)
}
async fn read_resource(app: &App, actor: &oauth::Actor, params: &Value) -> Result<Value> {
    let uri = params["uri"]
        .as_str()
        .ok_or_else(|| Error::invalid("Resource URI is required"))?;
    oauth::allowed(app, actor, "kdrive", false)?;
    let rest = uri
        .strip_prefix("kdrive://")
        .ok_or_else(|| Error::invalid("Unknown resource URI"))?;
    let (drive, file) = rest
        .split_once('/')
        .ok_or_else(|| Error::invalid("Invalid kDrive URI"))?;
    let drive = drive
        .parse::<i64>()
        .ok()
        .filter(|i| (1..=9007199254740991).contains(i))
        .ok_or_else(|| Error::invalid("Invalid drive ID"))?;
    let file = file
        .parse::<i64>()
        .ok()
        .filter(|i| (1..=9007199254740991).contains(i))
        .ok_or_else(|| Error::invalid("Invalid file ID"))?;
    let token = app
        .provider
        .access_token(app, &actor.user, "kdrive")
        .await?;
    oauth::allowed(app, actor, "kdrive", false)?;
    let metadata = file_metadata(
        data(
            app.provider
                .request(
                    &token,
                    reqwest::Method::GET,
                    format!("{}/3/drive/{drive}/files/{file}", app.provider.api),
                    &[],
                    None,
                )
                .await?,
        ),
        drive,
        Some(file),
        false,
    )?;
    let (_, bytes) = Provider::bytes(
        app.provider
            .http
            .get(format!(
                "{}/2/drive/{drive}/files/{file}/download",
                app.provider.api
            ))
            .bearer_auth(&token),
        false,
    )
    .await?;
    use base64::{Engine, engine::general_purpose::STANDARD};
    app.store.audit(
        "resource",
        Some(actor.user.id),
        Some(&actor.client),
        "kdrive.resource.read",
        "ok",
    )?;
    Ok(
        json!({"contents":[{"uri":uri,"mimeType":metadata["mime_type"].as_str().unwrap_or("application/octet-stream"),"blob":STANDARD.encode(bytes)}]}),
    )
}

async fn resource_call(c: Call, method: String) -> Result<Value> {
    match method.as_str() {
        "resources/list" => list_resources(&c.app, &c.actor, &c.args).await,
        "resources/read" => read_resource(&c.app, &c.actor, &c.args).await,
        "resources/templates/list" => Ok(
            json!({"resourceTemplates":[{"uriTemplate":"kdrive://{drive_id}/{file_id}","name":"kDrive file","description":"User-authorised files; numeric drive and file IDs"}]}),
        ),
        _ => Err(Error::invalid("Unknown resource method")),
    }
}
