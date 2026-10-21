//! Provider adapter: https://github.com/Infomaniak/mcp-server-mail/blob/main/src/mail-client.ts
use super::*;
use crate::identity::{Provider, binary};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::Method;

const MAX_OFFSET: i64 = 9007199254740991;

fn result(value: Value, context: Value) -> Value {
    super::result(mail_value(value), context)
}
fn mail_value(mut value: Value) -> Value {
    match &mut value {
        Value::Array(items) => {
            for item in items {
                *item = mail_value(std::mem::take(item));
            }
        }
        Value::Object(fields) => {
            for (key, item) in fields {
                if [
                    "id",
                    "uid",
                    "uuid",
                    "folder_id",
                    "thread_uid",
                    "first_message_uid",
                    "draft_uuid",
                    "identity_id",
                ]
                .contains(&key.as_str())
                    && item.is_number()
                {
                    *item = json!(item.to_string());
                } else {
                    *item = mail_value(std::mem::take(item));
                }
            }
        }
        _ => {}
    }
    value
}

fn message_metadata(value: Value, mailbox: &str, folder: &str, message: &str) -> Result<Value> {
    let mut value = mail_value(provider_object(value, false)?);
    let uid = |raw: &str| -> Option<String> {
        let id = if let Some((id, scope)) = raw.rsplit_once('@') {
            if scope != folder || id.contains('@') {
                return None;
            }
            id
        } else {
            raw
        };
        (safe_provider_id(id) && id.len() <= 256).then(|| id.to_owned())
    };
    let actual = value["uid"].as_str().and_then(uid);
    if actual.is_none()
        || actual != uid(message)
        || value.get("folder_id").is_some_and(|id| id != folder)
        || value.pointer("/folder/id").is_some_and(|id| id != folder)
        || value.get("mailbox_uuid").is_some_and(|id| id != mailbox)
    {
        return Err(Error::new(
            502,
            "provider_error",
            "Provider returned an invalid message, folder or mailbox identity",
        ));
    }
    value["folder_id"] = json!(folder);
    Ok(value)
}

fn mail_id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|id| safe_provider_id(id) && id.len() <= 256)
}

fn thread_summary(thread: &Value, mailbox: &str, default_folder: Option<&str>) -> Result<Value> {
    let invalid = || Error::new(502, "provider_error", "Invalid mail listing identity");
    let thread_uid = mail_id(&thread["uid"]).ok_or_else(invalid)?;
    if thread.get("mailbox_uuid").is_some_and(|id| id != mailbox)
        || thread
            .get("draft_uuid")
            .is_some_and(|id| !id.is_null() && mail_id(id).is_none())
        || thread
            .get("messages")
            .is_some_and(|messages| !messages.is_array())
    {
        return Err(invalid());
    }
    let message = thread.pointer("/messages/0");
    let (uid, folder) = if let Some(message) = message {
        if message.get("mailbox_uuid").is_some_and(|id| id != mailbox) {
            return Err(invalid());
        }
        let raw = mail_id(&message["uid"]).ok_or_else(invalid)?;
        let (uid, mut folder) = raw
            .rsplit_once('@')
            .map(|(uid, folder)| (uid, Some(folder)))
            .unwrap_or((raw, None));
        if !safe_provider_id(uid)
            || uid.contains('@')
            || folder.is_some_and(|folder| !safe_provider_id(folder))
        {
            return Err(invalid());
        }
        for candidate in [message.get("folder_id"), message.pointer("/folder/id")]
            .into_iter()
            .flatten()
        {
            let candidate = mail_id(candidate).ok_or_else(invalid)?;
            if folder.is_some_and(|folder| folder != candidate) {
                return Err(invalid());
            }
            folder = Some(candidate);
        }
        // Global search uses Inbox only as its provider query base, never as
        // the identity of a result whose actual folder is unknown.
        (
            Some(uid),
            Some(folder.or(default_folder).ok_or_else(invalid)?),
        )
    } else {
        (None, Some(default_folder.ok_or_else(invalid)?))
    };
    Ok(json!({
        "thread_uid":thread_uid,"subject":thread["subject"],"from":thread["from"],"to":thread["to"],
        "date":thread["date"],"messages_count":thread["messages_count"],"unseen_messages":thread["unseen_messages"],
        "seen":thread["unseen_messages"].as_i64().unwrap_or(0)==0,
        "flagged":thread["flagged"].as_bool().unwrap_or_else(||thread["messages"].as_array().is_some_and(|messages|messages.iter().any(|message|message["flagged"]==true))),
        "preview":message.map(|message| &message["preview"]),"first_message_uid":uid,"folder_id":folder,
        "folder":message.map(|message| &message["folder"]),"draft_uuid":thread["draft_uuid"]
    }))
}

pub fn module() -> Module {
    let compose = || {
        vec![
            ("to", id()),
            ("subject", s()),
            ("body", s()),
            ("cc", s()),
            ("bcc", s()),
            ("in_reply_to", id()),
            ("in_reply_to_uid", id()),
            ("references", s()),
            ("from_email", id()),
            ("from_name", id()),
        ]
    };
    let attachments = || {
        arr(
            json!({"type":"object","properties":{"filename":id(),"mime_type":id(),"base64_content":{"type":"string","maxLength":11184812}},"required":["filename","mime_type","base64_content"],"additionalProperties":false}),
        )
    };
    let mut send = compose();
    send.push(("attachments", attachments()));
    let mut tools = vec![
        tool(
            "mail_list_mailboxes",
            "List user-authorised mailboxes and primary status.",
            false,
            false,
            true,
            &[],
            vec![],
        ),
        tool(
            "mail_list_folders",
            "List flattened folders with identity, role and counts.",
            false,
            false,
            true,
            &[],
            vec![],
        ),
        tool(
            "mail_create_folder",
            "Create a folder, optionally under a parent.",
            true,
            false,
            true,
            &["name"],
            vec![("name", id()), ("parent_folder_id", path_id())],
        ),
        tool(
            "mail_rename_folder",
            "Rename a folder.",
            true,
            true,
            true,
            &["folder_id", "name"],
            vec![("folder_id", path_id()), ("name", id())],
        ),
        tool(
            "mail_delete_folder",
            "Delete a folder.",
            true,
            true,
            true,
            &["folder_id"],
            vec![("folder_id", path_id())],
        ),
        tool(
            "mail_list_emails",
            "List bounded thread summaries. unseen and flagged cannot both be true.",
            false,
            false,
            true,
            &["folder_id"],
            vec![
                ("folder_id", path_id()),
                (
                    "limit",
                    json!({"type":"integer","minimum":1,"maximum":100,"default":50}),
                ),
                (
                    "offset",
                    json!({"type":"integer","minimum":0,"maximum":MAX_OFFSET,"default":0}),
                ),
                ("unseen", b()),
                ("flagged", b()),
            ],
        ),
        tool(
            "mail_search_emails",
            "Search messages; omitted folder searches all folders. Requires a search criterion.",
            false,
            false,
            true,
            &[],
            vec![
                ("query", s()),
                ("from", s()),
                ("to", s()),
                ("subject", s()),
                ("since", id()),
                ("before", id()),
                ("folder_id", path_id()),
                (
                    "limit",
                    json!({"type":"integer","minimum":1,"maximum":100,"default":50}),
                ),
                (
                    "offset",
                    json!({"type":"integer","minimum":0,"maximum":MAX_OFFSET,"default":0}),
                ),
            ],
        ),
        tool(
            "mail_read_email",
            "Read bodies, headers, state and attachment metadata.",
            false,
            false,
            true,
            &["folder_id", "message_id"],
            vec![("folder_id", path_id()), ("message_id", path_id())],
        ),
        tool(
            "mail_download_attachment",
            "Download a message attachment as typed user-scoped content, up to 8 MiB.",
            false,
            false,
            true,
            &["folder_id", "message_id", "attachment_id"],
            vec![
                ("folder_id", path_id()),
                ("message_id", path_id()),
                ("attachment_id", path_id()),
            ],
        ),
        tool(
            "mail_send_email",
            "Create and send a message, reporting sending identity and any partial outcome. Attachments are inline filename, MIME type and base64 objects.",
            true,
            false,
            true,
            &["to", "subject", "body"],
            send,
        ),
        tool(
            "mail_create_draft",
            "Create a draft without sending; returns provider draft UUID and effective sending identity.",
            true,
            false,
            true,
            &["to", "subject", "body"],
            compose(),
        ),
        tool(
            "mail_list_drafts",
            "List provider draft threads and identifiers; continue with the returned next_offset.",
            false,
            false,
            true,
            &[],
            vec![
                (
                    "limit",
                    json!({"type":"integer","minimum":1,"maximum":100,"default":50}),
                ),
                (
                    "offset",
                    json!({"type":"integer","minimum":0,"maximum":MAX_OFFSET,"default":0}),
                ),
            ],
        ),
        tool(
            "mail_update_draft",
            "Edit an accessible provider draft without sending, preserving omitted fields.",
            true,
            true,
            true,
            &["draft_uuid"],
            vec![
                ("draft_uuid", path_id()),
                ("to", s()),
                ("subject", s()),
                ("body", s()),
                ("cc", s()),
                ("bcc", s()),
                (
                    "attachments",
                    json!({"type":"array","items":attachments()["items"],"maxItems":100}),
                ),
            ],
        ),
        tool(
            "mail_send_draft",
            "Send or schedule a provider draft; delay is in seconds.",
            true,
            true,
            true,
            &["draft_uuid"],
            vec![
                ("draft_uuid", path_id()),
                (
                    "delay",
                    json!({"type":"integer","minimum":0,"maximum":2592000,"default":0}),
                ),
            ],
        ),
        tool(
            "mail_delete_draft",
            "Delete an accessible draft.",
            true,
            true,
            true,
            &["draft_uuid"],
            vec![("draft_uuid", path_id())],
        ),
        tool(
            "mail_mark_email",
            "Mark nonempty, folder-scoped message UIDs read or unread.",
            true,
            true,
            true,
            &["folder_id", "message_ids", "read"],
            vec![
                ("folder_id", path_id()),
                ("message_ids", arr(id())),
                ("read", b()),
            ],
        ),
        tool(
            "mail_flag_email",
            "Star or unstar nonempty, folder-scoped message UIDs.",
            true,
            true,
            true,
            &["folder_id", "message_ids", "flagged"],
            vec![
                ("folder_id", path_id()),
                ("message_ids", arr(id())),
                ("flagged", b()),
            ],
        ),
        tool(
            "mail_move_email",
            "Move nonempty message UIDs between folders.",
            true,
            true,
            true,
            &["message_ids", "from_folder_id", "to_folder_id"],
            vec![
                ("message_ids", arr(id())),
                ("from_folder_id", path_id()),
                ("to_folder_id", path_id()),
            ],
        ),
        tool(
            "mail_archive_email",
            "Move messages to the Archive folder.",
            true,
            true,
            true,
            &["folder_id", "message_ids"],
            vec![("folder_id", path_id()), ("message_ids", arr(id()))],
        ),
        tool(
            "mail_delete_email",
            "Move messages to Trash by default. Permanent deletion requires permanent=true.",
            true,
            true,
            true,
            &["folder_id", "message_ids"],
            vec![
                ("folder_id", path_id()),
                ("message_ids", arr(id())),
                ("permanent", b()),
            ],
        ),
    ];
    for t in &mut tools {
        let object = json!({"type":"object","properties":{"uuid":{"type":["string","null"]},"uid":{"type":["string","number","null"]},"subject":{"type":["string","null"]},"body":{"type":["string","null"]},"html":{"type":["string","null"]},"headers":{"type":["object","array","null"]},"attachments":{"type":["array","null"]},"resolved_from":{"type":"object","properties":{"email":{"type":"string"},"name":{"type":["string","null"]},"identity_id":{"type":["string","number","null"]}}},"warnings":{"type":"array","items":{"type":"string"}}}});
        t.output = match t.name.as_str() {
            "mail_list_mailboxes" => {
                json!({"type":"array","items":{"type":"object","properties":{"uuid":{"type":"string"},"email":{"type":"string"},"mailbox":{"type":"string"},"is_primary":{"type":"boolean"}}}})
            }
            "mail_list_folders" => {
                json!({"type":"array","items":{"type":"object","properties":{"id":{"type":["string","integer"]},"name":{"type":"string"},"path":{"type":"string"},"role":{"type":["string","null"]},"unread_count":{"type":["integer","null"]},"total_count":{"type":["integer","null"]}}}})
            }
            "mail_list_emails" | "mail_search_emails" | "mail_list_drafts" => {
                json!({"type":"array","items":{"type":"object","properties":{"thread_uid":{"type":["string","null"]},"first_message_uid":{"type":["string","null"]},"folder_id":{"type":["string","integer","null"]},"subject":{"type":["string","null"]},"seen":{"type":"boolean"},"flagged":{"type":["boolean","null"]},"draft_uuid":{"type":["string","null"]}}}})
            }
            "mail_download_attachment" => {
                json!({"type":"object","properties":{"filename":{"type":"string"},"mime_type":{"type":"string"},"size":{"type":"integer"}},"required":["filename","mime_type","size"]})
            }
            _ => object,
        };
        if t.name != "mail_list_mailboxes" {
            t.schema["properties"]["mailbox_uuid"] = path_id();
        }
    }
    Module {
        id: "mail",
        name: "Mail",
        namespace: "mail",
        scopes: vec!["workspace:mail", "user_info"],
        tools,
        routes: vec![],
        resources: None,
        handler: |c, n| Box::pin(call(c, n)),
    }
}

fn flatten(folders: &[Value], prefix: &str, out: &mut Vec<Value>) {
    for folder in folders {
        let name = folder["name"].as_str().unwrap_or("");
        let path = if prefix.is_empty() {
            name.into()
        } else {
            format!(
                "{prefix}{}{name}",
                folder["separator"].as_str().unwrap_or("/")
            )
        };
        out.push(json!({"id":folder["id"],"name":name,"path":path,"role":folder["role"],"unread_count":folder["unread_count"],"total_count":folder["total_count"]}));
        if let Some(children) = folder["children"].as_array() {
            flatten(children, &path, out);
        }
    }
}
async fn folders(c: &Call, base: &str) -> Result<Vec<Value>> {
    let value = data(
        c.app
            .provider
            .request(
                &c.token,
                Method::GET,
                format!("{base}/folder"),
                &[("with".into(), "ik-static".into())],
                None,
            )
            .await?,
    );
    let mut out = vec![];
    flatten(
        value
            .as_array()
            .ok_or_else(|| Error::new(502, "provider_error", "Invalid folder listing"))?,
        "",
        &mut out,
    );
    Ok(out)
}
fn folder_role(folders: &[Value], role: &str) -> Result<String> {
    folders
        .iter()
        .find(|f| {
            f["role"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(role))
        })
        .and_then(|f| {
            f["id"]
                .as_str()
                .map(String::from)
                .or_else(|| f["id"].as_i64().map(|i| i.to_string()))
        })
        .filter(|id| safe_provider_id(id) && id.len() <= 256)
        .ok_or_else(|| {
            Error::new(
                502,
                "provider_error",
                "Required mailbox folder is unavailable",
            )
        })
}
pub fn scoped_uids(ids: &[Value], folder: &str) -> Result<Vec<String>> {
    ids.iter()
        .map(|id| {
            let id = id
                .as_str()
                .ok_or_else(|| Error::invalid("UID must be a string"))?;
            if let Some((uid, scope)) = id.rsplit_once('@') {
                if scope != folder || uid.is_empty() || uid.contains('@') {
                    return Err(Error::invalid("Message UID belongs to another folder"));
                }
                Ok(id.into())
            } else {
                Ok(format!("{id}@{folder}"))
            }
        })
        .collect()
}
fn recipients(raw: &str) -> Result<Value> {
    if raw.trim().is_empty() {
        return Ok(Value::Null);
    }
    let emails = raw.split(',').map(str::trim).collect::<Vec<_>>();
    if emails.len() > 100 || emails.iter().any(|e| !email(e)) {
        return Err(Error::invalid(
            "Recipients must be comma-separated email addresses",
        ));
    }
    Ok(json!(
        emails
            .iter()
            .map(|e| json!({"name":"","email":e}))
            .collect::<Vec<_>>()
    ))
}
fn header_text(value: &Value) -> bool {
    value.is_null()
        || value
            .as_str()
            .is_some_and(|s| !s.chars().any(char::is_control))
}
fn header_metadata(value: &Value) -> bool {
    match value {
        Value::String(_) => header_text(value),
        Value::Array(items) => items.iter().all(header_metadata),
        Value::Object(fields) => fields
            .iter()
            .all(|(key, value)| !key.chars().any(char::is_control) && header_metadata(value)),
        _ => true,
    }
}
fn header_address(value: &Value) -> bool {
    value.is_object() && value["email"].as_str().is_some_and(email) && header_text(&value["name"])
}
fn safe_headers(value: &Value) -> bool {
    [
        "subject",
        "from_name",
        "from_email",
        "in_reply_to",
        "in_reply_to_uid",
        "references",
    ]
    .iter()
    .all(|key| header_text(&value[key]))
        && ["from", "reply_to"]
            .iter()
            .all(|key| value[key].is_null() || header_address(&value[key]))
        && ["to", "cc", "bcc"].iter().all(|key| {
            let contacts = &value[key];
            header_text(contacts)
                || contacts
                    .as_array()
                    .is_some_and(|items| items.iter().all(header_address))
        })
        && header_metadata(&value["headers"])
        && (value["attachments"].is_null()
            || value["attachments"].as_array().is_some_and(|items| {
                items.iter().all(|item| {
                    (item.is_string() || item.is_object())
                        && header_metadata(item)
                        && ["filename", "name", "mime_type"]
                            .iter()
                            .all(|key| header_text(&item[key]))
                })
            }))
}
fn html(raw: &str) -> String {
    let escaped = raw
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('\n', "<br>");
    format!("<html><body>{escaped}</body></html>")
}
struct Attachment {
    bytes: Vec<u8>,
    filename: reqwest::header::HeaderValue,
    mime: reqwest::header::HeaderValue,
}
fn decode_attachments(args: &Value) -> Result<Vec<Attachment>> {
    let mut decoded = vec![];
    for att in args["attachments"].as_array().into_iter().flatten() {
        let bytes = STANDARD
            .decode(text(att, "base64_content")?)
            .map_err(|_| Error::invalid("Invalid attachment base64"))?;
        if bytes.len() > crate::identity::MAX_BYTES {
            return Err(Error::new(
                413,
                "content_too_large",
                "Attachment exceeds 8 MiB",
            ));
        }
        let header = |key| {
            reqwest::header::HeaderValue::from_str(text(att, key)?)
                .map_err(|_| Error::invalid("Invalid attachment filename or MIME type"))
        };
        decoded.push(Attachment {
            bytes,
            filename: header("filename")?,
            mime: header("mime_type")?,
        });
    }
    Ok(decoded)
}
fn draft_progress(c: &Call, draft: &str, phase: &str, uploaded: usize) -> Result<()> {
    c.set_mutation_context(json!({"draft_uuid":draft,"phase":phase,"progress":{"completed_attachment_uploads":uploaded},"recovery":"Inspect Drafts and Sent before repeating"}))
}
fn draft_failure(mut error: Error, draft: &str, phase: &str) -> Error {
    let context = json!({"draft_uuid":draft,"phase":phase,"cause":error.code,"progress":error.context.take(),"recovery":"Inspect Drafts and Sent before repeating"});
    if matches!(
        error.code,
        "invalid_token" | "access_denied" | "insufficient_scope" | "reconnection_required"
    ) {
        return error.with_context(context);
    }
    Error::new(
        502,
        if error.code == "mutation_uncertain" { "mutation_uncertain" } else { "partial_mutation" },
        "Draft operation failed or its outcome is uncertain. Inspect Drafts and Sent before repeating",
    ).with_context(context)
}
async fn attachments(
    c: &Call,
    base: &str,
    draft: &str,
    list: Vec<Attachment>,
) -> Result<Vec<String>> {
    let mut out = vec![];
    for att in list {
        draft_progress(c, draft, "upload_attachments", out.len())?;
        let failed =
            |error: Error| error.with_context(json!({"completed_attachment_uploads":out.len()}));
        let response = Provider::json(
            c.app
                .provider
                .http
                .post(format!("{base}/draft/attachment"))
                .bearer_auth(&c.token)
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-ws-attachment-disposition", "attachment")
                .header("x-ws-attachment-filename", att.filename)
                .header("x-ws-attachment-mime-type", att.mime)
                .body(att.bytes),
            true,
        )
        .await
        .map_err(failed)?;
        out.push(
            response["data"]["uuid"]
                .as_str()
                .filter(|id| safe_provider_id(id) && id.len() <= 256)
                .ok_or_else(|| {
                    failed(Error::new(
                        502,
                        "mutation_uncertain",
                        "Attachment upload outcome is uncertain",
                    ))
                })?
                .into(),
        );
        draft_progress(c, draft, "upload_attachments", out.len())?;
    }
    Ok(out)
}
pub fn attachment_filename(disposition: &str) -> Option<String> {
    let mut parameters = vec![];
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (i, ch) in disposition.char_indices() {
        if ch == '"' && !escaped {
            quoted = !quoted;
        }
        if ch == ';' && !quoted {
            parameters.push(&disposition[start..i]);
            start = i + 1;
        }
        escaped = ch == '\\' && !escaped;
    }
    parameters.push(&disposition[start..]);
    let mut fallback = None;
    for p in parameters {
        let Some((key, value)) = p.trim().split_once('=') else {
            continue;
        };
        if key.eq_ignore_ascii_case("filename*") {
            let parts = value.splitn(3, '\'').collect::<Vec<_>>();
            if parts.len() != 3 || !parts[0].eq_ignore_ascii_case("utf-8") {
                continue;
            }
            let bytes = parts[2].as_bytes();
            let mut decoded = vec![];
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%'
                    && i + 2 < bytes.len()
                    && let (Some(a), Some(b)) = (
                        (bytes[i + 1] as char).to_digit(16),
                        (bytes[i + 2] as char).to_digit(16),
                    )
                {
                    decoded.push((a * 16 + b) as u8);
                    i += 3;
                    continue;
                }
                decoded.push(bytes[i]);
                i += 1;
            }
            if let Ok(filename) = String::from_utf8(decoded) {
                return Some(filename);
            }
        } else if key.eq_ignore_ascii_case("filename") {
            fallback = Some(value.trim_matches('"').replace("\\\"", "\""));
        }
    }
    fallback
}
/// Returns the created draft and the exact payload the provider accepted for it.
async fn create_draft(c: &Call, mailbox: &Value, base: &str) -> Result<(Value, Value)> {
    let args = &c.args;
    let address = mailbox["email"]
        .as_str()
        .filter(|s| email(s))
        .ok_or_else(|| Error::new(502, "provider_error", "Mailbox email missing"))?;
    let requested = args["from_email"].as_str().unwrap_or(address);
    if !requested.eq_ignore_ascii_case(address)
        && !mailbox["aliases"].as_array().is_some_and(|a| {
            a.iter().any(|v| {
                v.as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(requested))
            })
        })
    {
        return Err(Error::denied());
    }
    let mut warnings = vec![];
    let mut identity = Value::Null;
    if mailbox
        .get("hosting_id")
        .is_some_and(|v| !v.is_null() && v.as_i64().is_none_or(|id| id <= 0))
        || mailbox.get("mailbox").is_some_and(|v| {
            !v.is_null()
                && v.as_str()
                    .is_none_or(|name| !safe_provider_id(name) || name.len() > 320)
        })
    {
        return Err(Error::new(
            502,
            "provider_error",
            "Invalid provider sending-identity lookup selectors",
        ));
    }
    if let (Some(hosting), Some(box_name)) =
        (mailbox["hosting_id"].as_i64(), mailbox["mailbox"].as_str())
    {
        let signatures = c
            .app
            .provider
            .request(
                &c.token,
                Method::GET,
                format!(
                    "{}/1/mail_hostings/{hosting}/mailboxes/{}/signatures",
                    c.app.provider.api,
                    segment(box_name)
                ),
                &[],
                None,
            )
            .await;
        match signatures {
            Ok(sigs) => {
                if let Some(candidates) = sigs.pointer("/data/signatures").and_then(Value::as_array)
                {
                    let candidates = candidates
                        .iter()
                        .filter(|s| {
                            s["sender"]
                                .as_str()
                                .is_some_and(|s| s.eq_ignore_ascii_case(requested))
                                && s["full_name"]
                                    .as_str()
                                    .is_some_and(|n| !n.trim().is_empty())
                        })
                        .collect::<Vec<_>>();
                    let choice = args["from_name"]
                        .as_str()
                        .and_then(|name| {
                            candidates.iter().find(|s| {
                                s["full_name"]
                                    .as_str()
                                    .is_some_and(|v| v.eq_ignore_ascii_case(name))
                            })
                        })
                        .or_else(|| candidates.iter().find(|s| s["is_default"] == true))
                        .or_else(|| candidates.first());
                    if let Some(s) = choice {
                        identity = (*s).clone();
                    }
                }
            }
            Err(error) if error.status == 401 || error.status == 403 => return Err(error),
            Err(_) => {
                warnings.push("Sending identities unavailable; provider default identity applies")
            }
        }
    } else {
        warnings.push("Sending identities unavailable; provider default identity applies");
    }
    let effective = identity["sender"].as_str().unwrap_or(address);
    let display = identity["full_name"].as_str().unwrap_or("");
    if !email(effective) || display.chars().any(char::is_control) {
        return Err(Error::new(
            502,
            "provider_error",
            "Invalid provider sending identity",
        ));
    }
    if !effective.eq_ignore_ascii_case(requested) {
        warnings.push("Requested alias has no sending identity; provider default address applies");
    }
    if args["from_name"]
        .as_str()
        .is_some_and(|n| !n.eq_ignore_ascii_case(display))
    {
        warnings.push("Display name comes from the provider identity or account profile");
    }
    let mut reply = args["in_reply_to"].clone();
    let mut refs = args["references"].clone();
    if let Some(uid) = args["in_reply_to_uid"].as_str() {
        let (message, folder) = uid
            .rsplit_once('@')
            .filter(|(m, f)| !m.is_empty() && !f.is_empty())
            .ok_or_else(|| Error::invalid("in_reply_to_uid must be UID@folder_id"))?;
        if !safe_provider_id(message) || !safe_provider_id(folder) {
            return Err(Error::invalid("Invalid reply source identifier"));
        }
        if reply.is_null() || refs.is_null() {
            let source = message_metadata(
                c.app
                    .provider
                    .request(
                        &c.token,
                        Method::GET,
                        format!(
                            "{base}/folder/{}/message/{}",
                            segment(folder),
                            segment(message)
                        ),
                        &[],
                        None,
                    )
                    .await?,
                text(mailbox, "uuid")?,
                folder,
                message,
            )?;
            let msg = source["msg_id"].as_str().ok_or_else(|| {
                Error::new(502, "provider_error", "Reply source has no Message-ID")
            })?;
            if reply.is_null() {
                reply = json!(msg)
            }
            if refs.is_null() {
                let previous = source
                    .pointer("/headers/references")
                    .or_else(|| source.pointer("/headers/References"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                refs = json!(format!("{previous} {msg}").trim());
            }
        }
    }
    let payload = json!({"uuid":null,"uid":null,"subject":args["subject"],"body":html(text(args,"body")?),"quote":null,"mime_type":"text/html","from":{"id":null,"email":effective,"name":display},"reply_to":{"email":effective,"name":display},"to":recipients(text(args,"to")?)?,"cc":recipients(args["cc"].as_str().unwrap_or(""))?,"bcc":recipients(args["bcc"].as_str().unwrap_or(""))?,"references":refs.as_str().unwrap_or(""),"in_reply_to":reply,"in_reply_to_uid":args["in_reply_to_uid"],"forwarded_uid":null,"attachments":[],"identity_id":identity["id"],"ack_request":false,"st_uuid":null,"resource":null,"priority":"normal","encrypted":false,"encryption_password":"","event_poll_uuid":null,"action":"save","delay":0});
    if !safe_headers(&payload) {
        return Err(Error::new(
            502,
            "provider_error",
            "Invalid provider reply headers",
        ));
    }
    c.set_mutation_context(json!({"mailbox_uuid":mailbox["uuid"],"phase":"create_draft","recovery":"Inspect Drafts before repeating"}))?;
    let response = c
        .app
        .provider
        .request(
            &c.token,
            Method::POST,
            format!("{base}/draft"),
            &[],
            Some(payload.clone()),
        )
        .await?;
    let mut draft = draft_metadata(
        provider_object(response, true)?,
        text(mailbox, "uuid")?,
        None,
        true,
    )?;
    draft["resolved_from"] = json!({"email":effective,"name":if display.is_empty(){Value::Null}else{json!(display)},"identity_id":identity["id"]});
    draft["warnings"] = json!(warnings);
    Ok((draft, payload))
}
/// Reduce a provider draft read to the writable payload the provider's own client sends.
fn editable_draft(read: &Value) -> Result<Value> {
    let invalid = || Error::new(502, "provider_error", "Draft cannot be resent unchanged");
    let person = |p: &Value| -> Result<Value> {
        let p = p.as_array().and_then(|a| a.first()).unwrap_or(p);
        if p.is_null() {
            return Ok(Value::Null);
        }
        Ok(
            json!({"id":p.get("id").cloned().unwrap_or(Value::Null),"email":p["email"].as_str().ok_or_else(invalid)?,"name":p["name"].as_str().unwrap_or("")}),
        )
    };
    let people = |v: &Value| -> Result<Value> {
        match v.as_array() {
            None => Ok(Value::Null),
            Some(list) if list.is_empty() => Ok(Value::Null),
            Some(list) => list
                .iter()
                .map(|p| Ok(json!({"name":p["name"].as_str().unwrap_or(""),"email":p["email"].as_str().ok_or_else(invalid)?})))
                .collect::<Result<Vec<_>>>()
                .map(Value::from),
        }
    };
    // Never drop an attachment silently: every entry needs an upload UUID.
    let attachments = read["attachments"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|a| {
                    a.as_str()
                        .or_else(|| a["uuid"].as_str())
                        .map(|id| json!(id))
                        .ok_or_else(invalid)
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(
        json!({"uuid":read["uuid"],"uid":read["uid"],"subject":read["subject"],"body":read["body"],"quote":read.get("quote").cloned().unwrap_or(Value::Null),"mime_type":read["mime_type"].as_str().unwrap_or("text/html"),"from":person(&read["from"])?,"reply_to":person(&read["reply_to"])?,"to":people(&read["to"])?,"cc":people(&read["cc"])?,"bcc":people(&read["bcc"])?,"references":read["references"].as_str().unwrap_or(""),"in_reply_to":read.get("in_reply_to").cloned().unwrap_or(Value::Null),"in_reply_to_uid":read.get("in_reply_to_uid").cloned().unwrap_or(Value::Null),"forwarded_uid":read.get("forwarded_uid").cloned().unwrap_or(Value::Null),"attachments":attachments,"identity_id":read.get("identity_id").cloned().unwrap_or(Value::Null),"ack_request":read["ack_request"].as_bool().unwrap_or(false),"st_uuid":read.get("st_uuid").cloned().unwrap_or(Value::Null),"resource":null,"priority":read["priority"].as_str().unwrap_or("normal"),"encrypted":false,"encryption_password":"","event_poll_uuid":null,"action":"save","delay":0}),
    )
}
fn draft_metadata(
    value: Value,
    mailbox: &str,
    expected: Option<&str>,
    mutation: bool,
) -> Result<Value> {
    let id = value["uuid"]
        .as_str()
        .filter(|id| safe_provider_id(id) && id.len() <= 256);
    // Creation/reads require an identity. Mutation acknowledgements may omit
    // one, but any returned identity must agree with the requested object.
    if !value.is_object()
        || ((expected.is_none() || !mutation) && id.is_none())
        || (value.get("uuid").is_some()
            && (id.is_none() || expected.is_some_and(|expected| id != Some(expected))))
        || value.get("mailbox_uuid").is_some_and(|id| id != mailbox)
    {
        let mut error = Error::new(
            502,
            if mutation {
                "mutation_uncertain"
            } else {
                "provider_error"
            },
            "Provider returned an invalid draft or mailbox identity; inspect Drafts and Sent before repeating",
        );
        if mutation && let Some(id) = id {
            error = error.with_context(json!({"returned_draft_uuid":id}));
        }
        return Err(error);
    }
    Ok(value)
}
async fn read_draft(c: &Call, base: &str, mailbox: &str, draft: &str) -> Result<Value> {
    let value = draft_metadata(
        provider_object(
            c.app
                .provider
                .request(
                    &c.token,
                    Method::GET,
                    format!("{base}/draft/{}", segment(draft)),
                    &[],
                    None,
                )
                .await?,
            false,
        )?,
        mailbox,
        Some(draft),
        false,
    )?;
    if !safe_headers(&value) {
        return Err(Error::new(
            502,
            "provider_error",
            "Invalid provider draft headers",
        ));
    }
    Ok(value)
}
pub async fn call(c: Call, name: String) -> Result<Value> {
    let args = &c.args;
    if matches!(
        name.as_str(),
        "mail_send_email" | "mail_create_draft" | "mail_update_draft"
    ) && !safe_headers(args)
    {
        return Err(Error::invalid(
            "Invalid mail headers or attachment metadata; control characters are forbidden",
        ));
    }
    // Reject every invalid attachment before creating a draft or uploading anything.
    let uploads = decode_attachments(args)?;
    let upload_count = uploads.len();
    let mailboxes = data(
        c.app
            .provider
            .request(
                &c.token,
                Method::GET,
                format!("{}/mailbox", c.app.provider.mail),
                &[(
                    "with".into(),
                    "aliases,permissions,accountId,count_users".into(),
                )],
                None,
            )
            .await?,
    );
    let boxes = mailboxes
        .as_array()
        .ok_or_else(|| Error::new(502, "provider_error", "Invalid mailbox list"))?;
    if name == "mail_list_mailboxes" {
        return Ok(result(mailboxes, json!({})));
    }
    let mailbox = if let Some(uuid) = args["mailbox_uuid"].as_str() {
        boxes.iter().find(|m| m["uuid"] == uuid)
    } else {
        boxes
            .iter()
            .find(|m| m["is_primary"] == true)
            .or_else(|| boxes.first())
    }
    .ok_or_else(Error::denied)?;
    let uuid = mailbox["uuid"]
        .as_str()
        .filter(|id| safe_provider_id(id) && id.len() <= 256)
        .ok_or_else(|| Error::new(502, "provider_error", "Mailbox UUID is missing or invalid"))?;
    let base = format!("{}/mail/{}", c.app.provider.mail, segment(uuid));
    let context = json!({"mailbox_uuid":uuid,"folder_id":args["folder_id"],"source_url":"https://mail.infomaniak.com"});
    let op = name
        .strip_prefix("mail_")
        .ok_or_else(|| Error::invalid("Unknown mail operation"))?;
    let folder = args["folder_id"].as_str().unwrap_or("");
    let folder_url = format!("{base}/folder/{}", segment(folder));
    let mut query = vec![];
    let (method, url, body) = match op {
        "list_folders" => return Ok(result(json!(folders(&c, &base).await?), context)),
        "create_folder" => (
            Method::POST,
            format!("{base}/folder"),
            Some(json!({"name":args["name"],"parent":args["parent_folder_id"]})),
        ),
        "rename_folder" => (
            Method::POST,
            format!("{folder_url}/rename"),
            Some(json!({"name":args["name"]})),
        ),
        "delete_folder" => (Method::DELETE, folder_url, None),
        "read_email" => {
            query.extend([
                ("prefered_format".into(), "html".into()),
                ("with".into(), "auto_uncrypt,thread_context".into()),
            ]);
            (
                Method::GET,
                format!(
                    "{folder_url}/message/{}",
                    segment(text(args, "message_id")?)
                ),
                None,
            )
        }
        "download_attachment" => {
            let response = c
                .app
                .provider
                .http
                .get(format!(
                    "{folder_url}/message/{}/attachment/{}",
                    segment(text(args, "message_id")?),
                    segment(text(args, "attachment_id")?)
                ))
                .bearer_auth(&c.token);
            let (headers, bytes) = Provider::bytes(response, false).await?;
            let mime = headers
                .get("content-type")
                .and_then(|s| s.to_str().ok())
                .unwrap_or("application/octet-stream")
                .split(';')
                .next()
                .unwrap_or("application/octet-stream");
            let filename = headers
                .get("content-disposition")
                .and_then(|s| s.to_str().ok())
                .and_then(attachment_filename)
                .unwrap_or_else(|| "attachment".into());
            let mut out = binary(
                format!(
                    "mail://{uuid}/{}/{}/{}",
                    segment(folder),
                    segment(text(args, "message_id")?),
                    segment(text(args, "attachment_id")?)
                ),
                filename,
                mime.into(),
                bytes,
            );
            let content = out.as_object_mut().unwrap().remove("content").unwrap();
            return Ok(json!({"data":out,"context":context,"content":content}));
        }
        "list_emails" | "search_emails" | "list_drafts" => {
            let search = op == "search_emails";
            let mut selected = folder.to_owned();
            if op == "list_drafts" {
                selected = folder_role(&folders(&c, &base).await?, "DRAFT")?;
            }
            if search {
                if !["query", "from", "to", "subject", "since", "before"]
                    .iter()
                    .any(|k| args[*k].as_str().is_some_and(|s| !s.trim().is_empty()))
                {
                    return Err(Error::invalid("At least one search criterion is required"));
                }
                if selected.is_empty() {
                    selected = folder_role(&folders(&c, &base).await?, "INBOX")?;
                }
                for (key, parameter) in [
                    ("query", "scontains"),
                    ("from", "sfrom"),
                    ("to", "sto"),
                    ("subject", "ssubject"),
                    ("since", "sfromdate"),
                    ("before", "stodate"),
                ] {
                    if let Some(raw) = args[key].as_str() {
                        if key == "since" || key == "before" {
                            let date = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                                .map_err(|_| Error::invalid("Search dates must be YYYY-MM-DD"))?;
                            if date.format("%Y-%m-%d").to_string() != raw {
                                return Err(Error::invalid("Search dates must be YYYY-MM-DD"));
                            }
                            query.push((parameter.into(), format!("{raw} 00:00:00")));
                        } else {
                            query.push((parameter.into(), raw.into()));
                        }
                    }
                }
            }
            if args["unseen"] == true && args["flagged"] == true {
                return Err(Error::invalid("unseen and flagged cannot both be true"));
            }
            if args["unseen"] == true {
                query.push(("filters".into(), "unseen".into()));
            } else if args["flagged"] == true {
                query.push(("filters".into(), "starred".into()));
            }
            let limit = numeric(args, "limit", 50);
            let offset = numeric(args, "offset", 0);
            query.extend([
                ("offset".into(), offset.to_string()),
                ("limit".into(), limit.to_string()),
                ("thread".into(), if search { "off" } else { "on" }.into()),
                (
                    "severywhere".into(),
                    if search && folder.is_empty() {
                        "1"
                    } else {
                        "0"
                    }
                    .into(),
                ),
            ]);
            let response = c
                .app
                .provider
                .request(
                    &c.token,
                    Method::GET,
                    format!("{base}/folder/{}/message", segment(&selected)),
                    &query,
                    None,
                )
                .await?;
            let threads = response
                .pointer("/data/threads")
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| Error::new(502, "provider_error", "Invalid message listing"))?;
            if threads.len() > limit as usize {
                return Err(Error::new(
                    502,
                    "provider_error",
                    "Provider exceeded the requested message page size",
                ));
            }
            let has_more = response["has_more"]
                .as_bool()
                .unwrap_or(threads.len() == limit as usize);
            let next_offset = offset + threads.len() as i64;
            if has_more && (threads.is_empty() || next_offset > MAX_OFFSET) {
                return Err(Error::new(
                    502,
                    "provider_error",
                    "Provider returned an incomplete message page without a usable continuation offset",
                ));
            }
            // Normalise before extracting UIDs: providers can return numeric IDs.
            let threads = mail_value(json!(threads));
            let threads = threads.as_array().ok_or_else(Error::internal)?;
            let default_folder = (!(search && folder.is_empty())).then_some(selected.as_str());
            let summaries = threads
                .iter()
                .map(|thread| thread_summary(thread, uuid, default_folder))
                .collect::<Result<Vec<_>>>()?;
            let mut out = result(json!(summaries), context);
            out["has_more"] = json!(has_more);
            out["cursor"] = if has_more {
                json!(next_offset.to_string())
            } else {
                Value::Null
            };
            out["next_offset"] = if has_more {
                json!(next_offset)
            } else {
                Value::Null
            };
            return Ok(out);
        }
        "create_draft" => return Ok(result(create_draft(&c, mailbox, &base).await?.0, context)),
        "send_email" => {
            // Send the composed payload the provider just accepted, as its own client does.
            let (draft, mut payload) = create_draft(&c, mailbox, &base).await?;
            let draft_uuid = text(&draft, "uuid")?;
            payload["uuid"] = json!(draft_uuid);
            payload["uid"] = draft.get("uid").cloned().unwrap_or(Value::Null);
            payload["resource"] = json!(format!("/api/mail/{uuid}/draft/{draft_uuid}"));
            if args.get("attachments").is_some() {
                payload["attachments"] = json!(
                    attachments(&c, &base, draft_uuid, uploads)
                        .await
                        .map_err(|e| draft_failure(e, draft_uuid, "upload_attachments"))?
                );
            }
            payload["action"] = json!("send");
            payload["delay"] = json!(0);
            draft_progress(&c, draft_uuid, "send", upload_count)?;
            let response = c
                .app
                .provider
                .request(
                    &c.token,
                    Method::PUT,
                    format!("{base}/draft/{}", segment(draft_uuid)),
                    &[],
                    Some(payload),
                )
                .await
                .map_err(|e| {
                    draft_failure(
                        e.with_context(json!({"completed_attachment_uploads":upload_count})),
                        draft_uuid,
                        "send",
                    )
                })?;
            return Ok(result(
                json!({"outcome":draft_metadata(data(response),uuid,Some(draft_uuid),true)?,"draft_uuid":draft_uuid,"resolved_from":draft["resolved_from"],"warnings":draft["warnings"]}),
                context,
            ));
        }
        "update_draft" | "send_draft" => {
            let draft = text(args, "draft_uuid")?;
            let url = format!("{base}/draft/{}", segment(draft));
            // Read from the provider, never a process-local cache, so drafts survive reconnection/restart.
            let mut payload = editable_draft(&read_draft(&c, &base, uuid, draft).await?)?;
            for key in ["to", "cc", "bcc"] {
                if let Some(raw) = args[key].as_str() {
                    payload[key] = recipients(raw)?;
                }
            }
            if let Some(raw) = args["subject"].as_str() {
                payload["subject"] = json!(raw);
            }
            if let Some(raw) = args["body"].as_str() {
                payload["body"] = json!(html(raw));
            }
            if args.get("attachments").is_some() {
                payload["attachments"] = json!(
                    attachments(&c, &base, draft, uploads)
                        .await
                        .map_err(|e| draft_failure(e, draft, "upload_attachments"))?
                );
            }
            payload["uuid"] = json!(draft);
            payload["resource"] = json!(format!("/api/mail/{uuid}/draft/{draft}"));
            payload["action"] = json!(if op == "send_draft" { "send" } else { "save" });
            payload["delay"] = json!(numeric(args, "delay", 0));
            draft_progress(
                &c,
                draft,
                if op == "send_draft" {
                    "send"
                } else {
                    "save_draft"
                },
                upload_count,
            )?;
            (Method::PUT, url, Some(payload))
        }
        "delete_draft" => (
            Method::DELETE,
            format!("{base}/draft/{}", segment(text(args, "draft_uuid")?)),
            None,
        ),
        "mark_email" | "flag_email" | "move_email" | "archive_email" | "delete_email" => {
            let from = if op == "move_email" {
                text(args, "from_folder_id")?
            } else {
                folder
            };
            let uids = scoped_uids(args["message_ids"].as_array().unwrap(), from)?;
            let (action, payload) = match op {
                "mark_email" => (
                    if args["read"] == true {
                        "seen"
                    } else {
                        "unseen"
                    },
                    json!({"uids":uids}),
                ),
                "flag_email" => (
                    if args["flagged"] == true {
                        "star"
                    } else {
                        "unstar"
                    },
                    json!({"uids":uids}),
                ),
                "delete_email" if args["permanent"] == true => {
                    ("delete", json!({"uids":uids,"move_reactions":false}))
                }
                _ => {
                    let target = if op == "move_email" {
                        text(args, "to_folder_id")?.into()
                    } else {
                        folder_role(
                            &folders(&c, &base).await?,
                            if op == "archive_email" {
                                "ARCHIVE"
                            } else {
                                "TRASH"
                            },
                        )?
                    };
                    (
                        "move",
                        json!({"uids":uids,"to":target,"move_reactions":false}),
                    )
                }
            };
            (
                Method::POST,
                format!("{base}/message/{action}"),
                Some(payload),
            )
        }
        _ => return Err(Error::invalid("Unknown mail operation")),
    };
    if method != Method::GET && op != "update_draft" && op != "send_draft" {
        c.set_mutation_context(json!({"mailbox_uuid":uuid,"folder_id":args["folder_id"],"parent_folder_id":args["parent_folder_id"],"from_folder_id":args["from_folder_id"],"to_folder_id":args["to_folder_id"],"message_ids":args["message_ids"],"draft_uuid":args["draft_uuid"],"phase":op,"recovery":"Inspect the mailbox before repeating"}))?;
    }
    let response = c
        .app
        .provider
        .request(&c.token, method, url, &query, body)
        .await
        .map_err(|e| {
            if op == "update_draft" || op == "send_draft" {
                draft_failure(
                    e.with_context(json!({"completed_attachment_uploads":upload_count})),
                    args["draft_uuid"].as_str().unwrap_or(""),
                    if op == "send_draft" {
                        "send"
                    } else {
                        "save_draft"
                    },
                )
            } else {
                e
            }
        })?;
    let mut value = if op == "read_email" {
        message_metadata(response, uuid, folder, text(args, "message_id")?)?
    } else if matches!(op, "update_draft" | "send_draft" | "delete_draft") {
        draft_metadata(data(response), uuid, Some(text(args, "draft_uuid")?), true)?
    } else {
        data(response)
    };
    if op == "create_folder" {
        let id = value["id"]
            .as_str()
            .map(String::from)
            .or_else(|| value["id"].as_i64().map(|id| id.to_string()));
        if id.is_none_or(|id| !safe_provider_id(&id) || id.len() > 256) {
            return Err(Error::new(
                502,
                "mutation_uncertain",
                "Folder creation result missing or invalid; inspect the parent folder before repeating",
            ));
        }
    }
    if op == "read_email"
        && let Some(attachments) = value["attachments"].as_array_mut()
    {
        for attachment in attachments {
            if !attachment.is_object() {
                return Err(Error::new(
                    502,
                    "provider_error",
                    "Invalid provider attachment metadata",
                ));
            }
            let id = attachment["resource"]
                .as_str()
                .and_then(|r| r.rsplit('/').next())
                .map(String::from)
                .or_else(|| attachment["part_id"].as_i64().map(|i| i.to_string()));
            attachment["id"] = id.map(Value::String).unwrap_or(Value::Null);
            attachment["filename"] = attachment
                .get("filename")
                .or_else(|| attachment.get("name"))
                .cloned()
                .unwrap_or(json!("attachment"));
            if attachment["mime_type"].is_null() {
                attachment["mime_type"] = json!("application/octet-stream");
            }
        }
    }
    Ok(result(value, context))
}
