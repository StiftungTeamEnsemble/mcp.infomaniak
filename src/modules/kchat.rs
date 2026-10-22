//! Provider adapter: https://github.com/Infomaniak/mcp-server-kchat/blob/main/src/kchat-client.ts
use super::*;
use reqwest::Method;

const MAX_PAGE: i64 = 10_000;
const MAX_ITEMS: usize = 1000;

fn result(mut value: Value, mut context: Value, team: &str) -> Value {
    let origin = format!("https://{team}.kchat.infomaniak.com/{team}");
    // Posts get the permalink kChat's "copy link" produces.
    let posts = match value.as_array_mut() {
        Some(list) => list.iter_mut().collect(),
        None => vec![&mut value],
    };
    for post in posts {
        if post.get("message").is_some()
            && let Some(id) = kid(&post["id"]).map(String::from)
        {
            post["source_url"] = json!(format!("{origin}/pl/{id}"));
        }
    }
    context["source_url"] = json!(origin);
    super::result(value, context)
}

pub fn module() -> Module {
    let limit =
        |default: i64| json!({"type":"integer","minimum":1,"maximum":100,"default":default});
    let page = || json!({"type":"integer","minimum":0,"maximum":MAX_PAGE,"default":0,"description":"Page from context.next_page; keep the other arguments unchanged"});
    let text = || json!({"type":"string","minLength":1,"maxLength":16383});
    let mut module = Module {
        id: "kchat",
        name: "kChat",
        namespace: "kchat",
        scopes: vec!["kchat"],
        routes: vec![],
        resources: None,
        handler: |c, n| Box::pin(call(c, n)),
        tools: vec![
            tool(
                "kchat_list_channels",
                "List public channels of the organisation's kChat team.",
                false,
                false,
                true,
                &[],
                vec![("limit", limit(100)), ("page", page())],
            ),
            tool(
                "kchat_list_my_channels",
                "List your channels, including private, direct and group channels.",
                false,
                false,
                true,
                &[],
                vec![],
            ),
            tool(
                "kchat_get_channel_history",
                "Read channel posts, newest first.",
                false,
                false,
                true,
                &["channel_id"],
                vec![
                    ("channel_id", path_id()),
                    ("limit", limit(10)),
                    ("page", page()),
                ],
            ),
            tool(
                "kchat_get_thread_replies",
                "Read a thread's root post and replies, oldest first.",
                false,
                false,
                true,
                &["thread_id"],
                vec![("thread_id", path_id())],
            ),
            tool(
                "kchat_get_users",
                "List kChat users with basic profiles.",
                false,
                false,
                true,
                &[],
                vec![("limit", limit(100)), ("page", page())],
            ),
            tool(
                "kchat_get_user_profile",
                "Read a kChat user's profile.",
                false,
                false,
                true,
                &["user_id"],
                vec![("user_id", path_id())],
            ),
            tool(
                "kchat_post_message",
                "Post a message to a channel as yourself.",
                true,
                false,
                true,
                &["channel_id", "text"],
                vec![("channel_id", path_id()), ("text", text())],
            ),
            tool(
                "kchat_reply_to_thread",
                "Reply to a thread as yourself; thread_id is the root post ID.",
                true,
                false,
                true,
                &["channel_id", "thread_id", "text"],
                vec![
                    ("channel_id", path_id()),
                    ("thread_id", path_id()),
                    ("text", text()),
                ],
            ),
            tool(
                "kchat_add_reaction",
                "Add an emoji reaction as yourself; use the emoji name without colons.",
                true,
                false,
                true,
                &["post_id", "reaction"],
                vec![("post_id", path_id()), ("reaction", id())],
            ),
            tool(
                "kchat_send_direct_message",
                "Send a direct message to a user by username as yourself.",
                true,
                false,
                true,
                &["username", "text"],
                vec![("username", id()), ("text", text())],
            ),
        ],
    };
    let id_type = json!({"type":"string"});
    let post = json!({"type":"object","properties":{"id":id_type,"channel_id":id_type,"root_id":id_type,"user_id":id_type,"message":{"type":"string"},"source_url":{"type":"string"},"create_at":{"type":"integer"},"update_at":{"type":"integer"}}});
    let channel = json!({"type":"object","properties":{"id":id_type,"team_id":id_type,"type":{"type":"string"},"name":{"type":"string"},"display_name":{"type":"string"},"purpose":{"type":"string"}}});
    let user = json!({"type":"object","properties":{"id":id_type,"username":{"type":"string"},"first_name":{"type":"string"},"last_name":{"type":"string"},"nickname":{"type":"string"},"email":{"type":"string"},"position":{"type":"string"}}});
    for tool in &mut module.tools {
        tool.output = match tool.name.as_str() {
            "kchat_list_channels" | "kchat_list_my_channels" => {
                json!({"type":"array","items":channel})
            }
            "kchat_get_channel_history" | "kchat_get_thread_replies" => {
                json!({"type":"array","items":post})
            }
            "kchat_get_users" => json!({"type":"array","items":user}),
            "kchat_get_user_profile" => user.clone(),
            "kchat_add_reaction" => {
                json!({"type":"object","properties":{"user_id":id_type,"post_id":id_type,"emoji_name":{"type":"string"},"create_at":{"type":"integer"}}})
            }
            _ => post.clone(),
        };
    }
    module
}

/// kChat object IDs are opaque alphanumeric strings or hyphenated UUIDs.
fn kid(value: &Value) -> Option<&str> {
    value.as_str().filter(|id| {
        !id.is_empty()
            && id.len() <= 64
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

fn arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    kid(&args[key]).ok_or_else(|| Error::invalid("Invalid kChat identifier"))
}

fn invalid(mutation: bool) -> Error {
    Error::new(
        502,
        if mutation {
            "mutation_uncertain"
        } else {
            "provider_error"
        },
        "Provider returned an invalid kChat identity; verify any mutation before repeating",
    )
}

fn incomplete() -> Error {
    Error::new(
        502,
        "provider_limitation",
        "Provider returned an incomplete or oversized list",
    )
}

struct Kchat<'a> {
    c: &'a Call,
    base: String,
}
impl Kchat<'_> {
    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Option<Value>,
    ) -> Result<Value> {
        self.c
            .app
            .provider
            .request(
                &self.c.token,
                method,
                format!("{}{path}", self.base),
                query,
                body,
            )
            .await
    }
    async fn get(&self, path: &str, query: &[(String, String)]) -> Result<Value> {
        self.send(Method::GET, path, query, None).await
    }
    async fn team_id(&self, team: &str) -> Result<String> {
        let found = self
            .get(&format!("/teams/name/{}", segment(team)), &[])
            .await?;
        if found["name"].as_str() != Some(team) {
            return Err(invalid(false));
        }
        kid(&found["id"])
            .map(String::from)
            .ok_or_else(|| invalid(false))
    }
    async fn me(&self) -> Result<String> {
        let me = self.get("/users/me", &[]).await?;
        kid(&me["id"])
            .map(String::from)
            .ok_or_else(|| invalid(false))
    }
    async fn post(&self, channel: &str, thread: Option<&str>, text: &str) -> Result<Value> {
        let mut body = json!({"channel_id":channel,"message":text});
        if let Some(thread) = thread {
            body["root_id"] = json!(thread);
        }
        let post = self.send(Method::POST, "/posts", &[], Some(body)).await?;
        let id = kid(&post["id"]).map(String::from);
        if id.is_none()
            || kid(&post["channel_id"]) != Some(channel)
            || post["root_id"].as_str().unwrap_or("") != thread.unwrap_or("")
        {
            let mut error = invalid(true);
            if let Some(id) = id {
                error = error.with_context(json!({"returned_post_id":id}));
            }
            return Err(error);
        }
        Ok(post)
    }
}

/// Posts in the provider's order; each must match its key and the requested channel/thread.
fn posts(list: &Value, channel: Option<&str>, thread: Option<&str>) -> Result<Vec<Value>> {
    let order = list["order"].as_array().ok_or_else(|| invalid(false))?;
    if order.len() > MAX_ITEMS {
        return Err(incomplete());
    }
    order
        .iter()
        .map(|key| {
            let key = kid(key).ok_or_else(|| invalid(false))?;
            let post = &list["posts"][key];
            if kid(&post["id"]) != Some(key)
                || kid(&post["channel_id"]).is_none()
                || channel.is_some_and(|c| kid(&post["channel_id"]) != Some(c))
                || thread.is_some_and(|t| key != t && post["root_id"].as_str() != Some(t))
            {
                return Err(invalid(false));
            }
            Ok(post.clone())
        })
        .collect()
}

fn items(list: Value, valid: impl Fn(&Value) -> bool) -> Result<Vec<Value>> {
    let list = list.as_array().cloned().ok_or_else(|| invalid(false))?;
    if list.len() > MAX_ITEMS {
        return Err(incomplete());
    }
    if !list
        .iter()
        .all(|item| kid(&item["id"]).is_some() && valid(item))
    {
        return Err(invalid(false));
    }
    Ok(list)
}

fn paging(args: &Value, default: i64) -> (i64, i64, Vec<(String, String)>) {
    let (limit, page) = (numeric(args, "limit", default), numeric(args, "page", 0));
    let query = vec![
        ("page".into(), page.to_string()),
        ("per_page".into(), limit.to_string()),
    ];
    (limit, page, query)
}

fn paged(items: Vec<Value>, more: bool, page: i64, context: Value, team: &str) -> Result<Value> {
    if more && page >= MAX_PAGE {
        return Err(incomplete());
    }
    let mut out = result(json!(items), context, team);
    out["has_more"] = json!(more);
    out["context"]["next_page"] = if more { json!(page + 1) } else { Value::Null };
    Ok(out)
}

pub async fn call(c: Call, name: String) -> Result<Value> {
    let team = c.app.config.kchat_team.clone().ok_or_else(|| {
        Error::new(
            503,
            "module_unconfigured",
            "kChat team is not configured on this server",
        )
    })?;
    let k = Kchat {
        c: &c,
        base: format!("{}/api/v4", c.app.provider.kchat.replace("{team}", &team)),
    };
    let args = &c.args;
    match name.as_str() {
        "kchat_list_channels" => {
            let team_id = k.team_id(&team).await?;
            let (limit, page, query) = paging(args, 100);
            let list = items(
                k.get(&format!("/teams/{team_id}/channels"), &query).await?,
                |ch| ch["team_id"] == team_id.as_str(),
            )?;
            let more = list.len() as i64 >= limit;
            paged(list, more, page, json!({"team_id":team_id}), &team)
        }
        "kchat_list_my_channels" => {
            let team_id = k.team_id(&team).await?;
            // Direct and group channels belong to no team.
            let list = items(
                k.get(&format!("/users/me/teams/{team_id}/channels"), &[])
                    .await?,
                |ch| ch["team_id"] == team_id.as_str() || ch["team_id"] == "",
            )?;
            Ok(result(json!(list), json!({"team_id":team_id}), &team))
        }
        "kchat_get_channel_history" => {
            let channel = arg(args, "channel_id")?;
            let (limit, page, query) = paging(args, 10);
            let list = k.get(&format!("/channels/{channel}/posts"), &query).await?;
            let found = posts(&list, Some(channel), None)?;
            let more = list["has_next"]
                .as_bool()
                .unwrap_or(found.len() as i64 >= limit);
            paged(found, more, page, json!({"channel_id":channel}), &team)
        }
        "kchat_get_thread_replies" => {
            let thread = arg(args, "thread_id")?;
            let list = k.get(&format!("/posts/{thread}/thread"), &[]).await?;
            if list["has_next"] == true {
                return Err(incomplete());
            }
            let mut found = posts(&list, None, Some(thread))?;
            let channel = found
                .iter()
                .find(|p| p["id"] == thread)
                .map(|p| p["channel_id"].clone())
                .ok_or_else(|| invalid(false))?;
            if found.iter().any(|p| p["channel_id"] != channel) {
                return Err(invalid(false));
            }
            found.sort_by_key(|p| p["create_at"].as_i64().unwrap_or(0));
            Ok(result(
                json!(found),
                json!({"thread_id":thread,"channel_id":channel}),
                &team,
            ))
        }
        "kchat_get_users" => {
            let (limit, page, query) = paging(args, 100);
            let list = items(k.get("/users", &query).await?, |_| true)?;
            let more = list.len() as i64 >= limit;
            paged(list, more, page, json!({}), &team)
        }
        "kchat_get_user_profile" => {
            let user = arg(args, "user_id")?;
            let found = k.get(&format!("/users/{user}"), &[]).await?;
            if kid(&found["id"]) != Some(user) {
                return Err(invalid(false));
            }
            Ok(result(found, json!({"user_id":user}), &team))
        }
        "kchat_post_message" | "kchat_reply_to_thread" => {
            let channel = arg(args, "channel_id")?;
            let thread = if name == "kchat_reply_to_thread" {
                Some(arg(args, "thread_id")?)
            } else {
                None
            };
            c.set_mutation_context(json!({"channel_id":channel,"thread_id":thread,"phase":"post_message","recovery":"Check the channel before repeating"}))?;
            let post = k.post(channel, thread, text(args, "text")?).await?;
            Ok(result(
                post,
                json!({"channel_id":channel,"thread_id":thread}),
                &team,
            ))
        }
        "kchat_add_reaction" => {
            let post = arg(args, "post_id")?;
            let reaction = text(args, "reaction")?.trim_matches(':');
            if reaction.is_empty()
                || reaction.len() > 64
                || !reaction
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_+-".contains(&b))
            {
                return Err(Error::invalid("Use an emoji name such as thumbsup"));
            }
            let me = k.me().await?;
            c.set_mutation_context(json!({"post_id":post,"reaction":reaction,"phase":"add_reaction","recovery":"Check the post's reactions before repeating"}))?;
            let added = k
                .send(
                    Method::POST,
                    "/reactions",
                    &[],
                    Some(json!({"user_id":me,"post_id":post,"emoji_name":reaction})),
                )
                .await?;
            if kid(&added["post_id"]) != Some(post)
                || added["emoji_name"] != reaction
                || added
                    .get("user_id")
                    .is_some_and(|u| kid(u) != Some(me.as_str()))
            {
                return Err(invalid(true));
            }
            Ok(result(added, json!({"post_id":post}), &team))
        }
        "kchat_send_direct_message" => {
            let username = text(args, "username")?
                .trim()
                .trim_start_matches('@')
                .to_ascii_lowercase();
            if username.is_empty()
                || !username
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            {
                return Err(Error::invalid("Invalid kChat username"));
            }
            let me = k.me().await?;
            let recipient = k
                .get(&format!("/users/username/{}", segment(&username)), &[])
                .await?;
            let user = kid(&recipient["id"])
                .filter(|_| {
                    recipient["username"]
                        .as_str()
                        .is_some_and(|u| u.eq_ignore_ascii_case(&username))
                })
                .ok_or_else(|| invalid(false))?;
            c.set_mutation_context(json!({"username":username,"user_id":user,"phase":"open_direct_channel","recovery":"Check your direct messages before repeating"}))?;
            let channel = k
                .send(
                    Method::POST,
                    "/channels/direct",
                    &[],
                    Some(json!([me, user])),
                )
                .await?;
            let channel = kid(&channel["id"])
                .filter(|_| channel["type"] == "D")
                .ok_or_else(|| invalid(true))?;
            c.set_mutation_context(json!({"username":username,"user_id":user,"channel_id":channel,"phase":"post_message","completed":["open_direct_channel"],"recovery":"Check the direct channel before repeating"}))?;
            let post = k.post(channel, None, text(args, "text")?).await?;
            Ok(result(
                post,
                json!({"channel_id":channel,"user_id":user}),
                &team,
            ))
        }
        _ => Err(Error::invalid("Unknown tool")),
    }
}
