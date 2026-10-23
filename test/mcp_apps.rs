use super::*;

#[tokio::test]
async fn app_resources_are_module_owned_scoped_and_static() {
    let h = Harness::new().await;
    let tools = h.rpc("tools/list", json!({})).await.value;
    let tools = tools["result"]["tools"].as_array().unwrap();
    let apps = tools
        .iter()
        .filter(|t| t["_meta"]["ui"]["resourceUri"].is_string())
        .collect::<Vec<_>>();
    assert_eq!(apps.len(), 4);
    for tool in apps {
        let uri = tool["_meta"]["ui"]["resourceUri"].as_str().unwrap();
        assert_eq!(tool["_meta"]["openai/outputTemplate"], uri);
        assert_eq!(tool["_meta"]["openai/widgetAccessible"], true);
        assert_eq!(tool["_meta"]["ui"]["visibility"], json!(["model", "app"]));
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        let resource = h.rpc("resources/read", json!({"uri":uri})).await;
        let content = &resource.value["result"]["contents"][0];
        assert_eq!(content["uri"], uri);
        assert_eq!(content["mimeType"], "text/html;profile=mcp-app");
        assert!(
            content["text"]
                .as_str()
                .unwrap()
                .starts_with("<!doctype html>")
        );
        assert_eq!(content["_meta"]["ui"]["csp"]["connectDomains"], json!([]));
        assert!(!content["text"].as_str().unwrap().contains("delegated-a"));
    }
    assert!(h.mock.lock().unwrap().calls.is_empty());
    assert!(
        h.rpc("resources/read", json!({"uri":"ui://ensemble/object.html"}))
            .await
            .value["error"]
            .is_object()
    );
    h.sql("UPDATE grants SET scopes='mail:read'");
    let resources = h.rpc("resources/list", json!({})).await.value;
    assert_eq!(
        resources["result"]["resources"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        resources["result"]["resources"][0]["uri"],
        "ui://mail/message-v1.html"
    );
    let denied = h
        .rpc(
            "resources/read",
            json!({"uri":"ui://kchat/message-v1.html"}),
        )
        .await;
    assert_eq!(denied.status, 403);
    assert!(
        denied.headers["www-authenticate"]
            .to_str()
            .unwrap()
            .contains("kchat:read")
    );
    h.sql("UPDATE modules SET enabled=0 WHERE module='mail'");
    assert_eq!(
        h.rpc("resources/read", json!({"uri":"ui://mail/message-v1.html"}))
            .await
            .status,
        403
    );
    h.sql("DELETE FROM tokens");
    assert_eq!(
        h.rpc("resources/read", json!({"uri":"ui://mail/message-v1.html"}))
            .await
            .status,
        401
    );
}

#[test]
fn app_registration_rejects_cross_module_and_duplicate_resources() {
    let mut module = modules::mail::module();
    let show = module
        .tools
        .iter_mut()
        .find(|t| t.name == "mail_show_email")
        .unwrap();
    show.ui.as_mut().unwrap().uri = "ui://kchat/message-v1.html";
    assert!(modules::Registry::new(vec![module]).is_err());
    let mut module = modules::mail::module();
    let mut duplicate = module
        .tools
        .iter()
        .find(|t| t.name == "mail_show_email")
        .unwrap()
        .clone();
    duplicate.name = "mail_duplicate".into();
    module.tools.push(duplicate);
    assert!(modules::Registry::new(vec![module]).is_err());
}

#[tokio::test]
async fn message_and_folder_apps_reuse_validated_read_paths() {
    let h = Harness::new().await;
    h.mock.lock().unwrap().responses.insert("GET /mail/box-a/folder/inbox/message/12".into(), json!({"data":{"uid":"12@inbox","folder_id":"inbox","subject":"Private subject","body":"Private body","mime_type":"text/plain"}}));
    let args = json!({"folder_id":"inbox","message_id":"12"});
    let shown = h.call("mail_show_email", args.clone()).await;
    let read = h.call("mail_read_email", args).await;
    assert_eq!(shown["result"]["isError"], false, "{shown}");
    assert_eq!(result(&shown), result(&read));
    h.mock.lock().unwrap().responses.insert(
        "GET /mail/box-a/folder/inbox/message/12".into(),
        json!({"data":{"uid":"99@inbox","body":"Wrong message"}}),
    );
    let invalid = h
        .call(
            "mail_show_email",
            json!({"folder_id":"inbox","message_id":"12"}),
        )
        .await;
    assert_eq!(invalid["result"]["_meta"]["error_code"], "provider_error");

    let shown = h
        .call("kdrive_show_folder", json!({"drive_id":9,"file_id":1}))
        .await;
    assert_eq!(shown["result"]["isError"], false);
    assert!(result(&shown)["context"]["folder_name"].is_null());
    h.mock.lock().unwrap().responses.insert(
        "GET /3/drive/9/files/1".into(),
        json!({"data":{"id":1,"name":"Projects","type":"dir"}}),
    );
    let shown = h
        .call("kdrive_show_folder", json!({"drive_id":9,"file_id":1}))
        .await;
    assert_eq!(result(&shown)["context"]["folder_name"], "Projects");
    assert_eq!(result(&shown)["context"]["drive_id"], 9);
    assert_eq!(result(&shown)["context"]["file_id"], 1);
    assert_eq!(result(&shown)["data"][0]["id"], 42);
    h.mock.lock().unwrap().responses.insert(
        "GET /3/drive/9/files/1/files".into(),
        json!({"data":[{"id":42,"drive_id":8,"name":"Wrong drive"}]}),
    );
    let invalid = h.call("kdrive_show_folder", json!({"drive_id":9})).await;
    assert_eq!(invalid["result"]["_meta"]["error_code"], "provider_error");
    let logs = h
        .app
        .store
        .with(|db| {
            Ok(db
                .prepare("SELECT outcome FROM logs")?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
                .join("\n"))
        })
        .unwrap();
    assert!(!logs.contains("Private subject"));
    assert!(!logs.contains("Private body"));
}

#[tokio::test]
async fn kchat_message_app_validates_post_channel_user_and_transport_errors() {
    let h = Harness::new().await;
    let path = "GET /kchat/ensemble/api/v4/posts/post1";
    let post = json!({"id":"post1","channel_id":"chan1","user_id":"me1","root_id":"","message":"Hello","create_at":1});
    h.mock
        .lock()
        .unwrap()
        .responses
        .insert(path.into(), post.clone());
    let shown = h
        .call(
            "kchat_show_message",
            json!({"post_id":"post1","channel_id":"chan1"}),
        )
        .await;
    assert_eq!(shown["result"]["isError"], false, "{shown}");
    assert_eq!(
        result(&shown)["data"]["source_url"],
        "https://ensemble.kchat.infomaniak.com/ensemble/pl/post1"
    );
    assert!(result(&shown)["context"]["author"].is_null());
    {
        let mut mock = h.mock.lock().unwrap();
        mock.responses.insert(
            "GET /kchat/ensemble/api/v4/users/me1".into(),
            json!({"id":"me1","username":"lara","first_name":"Lara","last_name":"Muster"}),
        );
        mock.responses.insert(
            "GET /kchat/ensemble/api/v4/channels/chan1".into(),
            json!({"id":"chan1","display_name":"Kommunikation","type":"O"}),
        );
    }
    let shown = h
        .call("kchat_show_message", json!({"post_id":"post1"}))
        .await;
    assert_eq!(
        result(&shown)["context"]["author"],
        json!({"username":"lara","name":"Lara Muster"})
    );
    assert_eq!(
        result(&shown)["context"]["channel"],
        json!({"name":"Kommunikation","type":"O"})
    );
    // Optional names never hide an invalid token.
    h.mock.lock().unwrap().response_statuses.insert(
        "GET /kchat/ensemble/api/v4/users/me1".into(),
        StatusCode::UNAUTHORIZED,
    );
    assert_eq!(
        h.call("kchat_show_message", json!({"post_id":"post1"}))
            .await["error"],
        "reconnection_required"
    );
    h.mock.lock().unwrap().response_statuses.clear();
    for (field, value) in [
        ("id", json!("post2")),
        ("channel_id", json!("chan2")),
        ("user_id", json!("../me")),
        ("root_id", json!("../root")),
        ("message", Value::Null),
    ] {
        let mut invalid = post.clone();
        invalid[field] = value;
        h.mock
            .lock()
            .unwrap()
            .responses
            .insert(path.into(), invalid);
        let shown = h
            .call(
                "kchat_show_message",
                json!({"post_id":"post1","channel_id":"chan1"}),
            )
            .await;
        assert_eq!(
            shown["result"]["_meta"]["error_code"], "provider_error",
            "{shown}"
        );
    }
    h.mock.lock().unwrap().responses.remove(path);
    h.mock
        .lock()
        .unwrap()
        .response_statuses
        .insert(path.into(), StatusCode::FORBIDDEN);
    assert_eq!(
        h.rpc(
            "tools/call",
            json!({"name":"kchat_show_message","arguments":{"post_id":"post1"}})
        )
        .await
        .status,
        403
    );
}

fn availability_args() -> Value {
    json!({"calendar_ids":["alice","bob"],"from":"2026-10-22","to":"2026-10-23","duration_minutes":30})
}

fn event(id: i64, calendar: &str, start: &str, end: &str) -> Value {
    json!({"id":id,"calendar_id":calendar,"start":start,"end":end,"timezone_start":"Europe/Zurich","timezone_end":"Europe/Zurich","title":"Private meeting title"})
}

#[tokio::test]
async fn common_availability_merges_all_calendars_and_omits_short_gaps_and_free_events() {
    let h = Harness::new().await;
    h.mock.lock().unwrap().responses.insert(
        "GET /1/calendar/pim/calendar".into(),
        json!({"data":{"calendars":[{"id":"alice","name":"Alice"},{"id":"bob","name":"Bob"}]}}),
    );
    let mut free = event(4, "bob", "2026-10-22 14:00:00", "2026-10-22 16:00:00");
    free["freebusy"] = json!("free");
    h.mock.lock().unwrap().queued_responses.insert("GET /1/calendar/pim/event".into(), VecDeque::from([
        (StatusCode::OK,json!({"data":[event(1,"alice","2026-10-22 10:00:00","2026-10-22 11:00:00")]})),
        (StatusCode::OK,json!({"data":[event(2,"bob","2026-10-22T08:30:00Z","2026-10-22T10:00:00Z"),event(3,"bob","2026-10-22 12:20:00","2026-10-22 13:00:00"),free]})),
    ]));
    let shown = h
        .call("calendar_find_free_slots", availability_args())
        .await;
    assert_eq!(shown["result"]["isError"], false, "{shown}");
    let data = &result(&shown)["data"];
    assert_eq!(
        data["slots"],
        json!([
            {"start":"2026-10-22T09:00:00+02:00","end":"2026-10-22T10:00:00+02:00","duration_minutes":60},
            {"start":"2026-10-22T13:00:00+02:00","end":"2026-10-22T17:00:00+02:00","duration_minutes":240}
        ])
    );
    assert_eq!(data["calendars"].as_array().unwrap().len(), 2);
    assert_eq!(data["calendars"][0]["name"], "Alice");
    assert_eq!(data["calendars"][1]["name"], "Bob");
    assert!(!shown.to_string().contains("Private meeting title"));
    assert!(!h.mock.lock().unwrap().calls.iter().any(|c| c.0 != "GET"));
}

#[tokio::test]
async fn availability_fails_closed_for_incomplete_invalid_or_recurring_events() {
    for response in [
        json!({"data":[],"has_more":true}),
        json!({"data":[event(1,"wrong","2026-10-22 10:00:00","2026-10-22 11:00:00")]}),
        json!({"data":[{"id":1,"calendar_id":"alice","start":"garbled","end":"garbled"}]}),
        json!({"data":[{"id":1,"calendar_id":"alice","start":"2026-10-22 10:00:00","end":"2026-10-22 10:00:00"}]}),
        json!({"data":[{"id":1,"calendar_id":"alice","rrule":"FREQ=DAILY","start":"2026-10-22 10:00:00","end":"2026-10-22 11:00:00"}]}),
    ] {
        let h = Harness::new().await;
        h.mock
            .lock()
            .unwrap()
            .responses
            .insert("GET /1/calendar/pim/event".into(), response);
        let out = h
            .call("calendar_find_free_slots", availability_args())
            .await;
        assert_eq!(out["result"]["isError"], true, "{out}");
        assert!(out["result"].get("structuredContent").is_none());
    }
    let h = Harness::new().await;
    h.mock.lock().unwrap().queued_responses.insert(
        "GET /1/calendar/pim/event".into(),
        VecDeque::from([
            (StatusCode::OK, json!({"data":[]})),
            (StatusCode::FORBIDDEN, json!({})),
        ]),
    );
    let out = h
        .rpc(
            "tools/call",
            json!({"name":"calendar_find_free_slots","arguments":availability_args()}),
        )
        .await;
    assert_eq!(out.status, 403);
}

#[tokio::test]
async fn availability_uses_local_workdays_all_day_events_and_dst_offsets() {
    let h = Harness::new().await;
    h.mock.lock().unwrap().responses.insert(
        "GET /1/calendar/pim/event".into(),
        json!({"data":[event(1,"alice","2026-10-23","2026-10-24")]}),
    );
    let out = h
        .call(
            "calendar_find_free_slots",
            json!({"calendar_ids":["alice"],"from":"2026-10-23","to":"2026-10-27"}),
        )
        .await;
    assert_eq!(out["result"]["isError"], false, "{out}");
    assert_eq!(
        result(&out)["data"]["slots"],
        json!([{"start":"2026-10-26T09:00:00+01:00","end":"2026-10-26T17:00:00+01:00","duration_minutes":480}])
    );
    let mut args = availability_args();
    args["from"] = json!("2026-10-25T02:15:00+02:00");
    args["to"] = json!("2026-10-25T02:45:00+01:00");
    let out = h.call("calendar_find_free_slots", args).await;
    assert_eq!(out["result"]["_meta"]["error_code"], "provider_limitation");
}

#[tokio::test]
async fn availability_validates_actual_tool_bounds_and_duplicate_calendars() {
    let h = Harness::new().await;
    for (field, value) in [
        ("calendar_ids", json!([])),
        ("calendar_ids", json!(["../other"])),
        ("duration_minutes", json!(4)),
        ("duration_minutes", json!(1441)),
        ("weekdays", json!([0])),
        ("day_start", json!("9:00")),
    ] {
        let mut args = availability_args();
        args[field] = value;
        let out = h.call("calendar_find_free_slots", args).await;
        assert_eq!(out["error"]["code"], -32602, "{out}");
    }
    assert!(h.mock.lock().unwrap().calls.is_empty());
    for (field, value) in [
        ("calendar_ids", json!(["alice", "alice"])),
        ("day_start", json!("99:00")),
        ("day_end", json!("08:00")),
        ("to", json!("2027-01-01")),
    ] {
        let mut args = availability_args();
        args[field] = value;
        let out = h.call("calendar_find_free_slots", args).await;
        assert_eq!(
            out["result"]["_meta"]["error_code"], "invalid_request",
            "{out}"
        );
    }
}

#[tokio::test]
async fn availability_continuation_is_accepted_without_skipping_free_windows() {
    let h = Harness::new().await;
    let mut events = vec![];
    for day in 1..=21 {
        for hour in 9..17 {
            events.push(event(
                events.len() as i64 + 1,
                "alice",
                &format!("2026-10-{day:02} {hour:02}:15:00"),
                &format!("2026-10-{day:02} {hour:02}:30:00"),
            ));
        }
    }
    h.mock
        .lock()
        .unwrap()
        .responses
        .insert("GET /1/calendar/pim/event".into(), json!({"data":events}));
    let mut args = json!({"calendar_ids":["alice"],"from":"2026-10-01","to":"2026-10-22","weekdays":[1,2,3,4,5,6,7],"duration_minutes":5});
    let first = h.call("calendar_find_free_slots", args.clone()).await;
    assert_eq!(first["result"]["isError"], false, "{first}");
    assert_eq!(
        result(&first)["data"]["slots"].as_array().unwrap().len(),
        100
    );
    assert_eq!(result(&first)["has_more"], true);
    args["from"] = result(&first)["context"]["next_from"].clone();
    let second = h.call("calendar_find_free_slots", args.clone()).await;
    assert_eq!(second["result"]["isError"], false, "{second}");
    assert_eq!(result(&second)["data"]["slots"][0]["start"], args["from"]);
    assert_eq!(result(&second)["has_more"], false);
    assert_eq!(
        result(&second)["data"]["slots"].as_array().unwrap().len(),
        89
    );
}

#[tokio::test]
async fn kdrive_lists_accessible_drives_and_separates_wrong_drives_from_revoked_tokens() {
    let h = Harness::new().await;
    h.mock.lock().unwrap().responses.insert(
        "GET /2/drive/init?with=drives".into(),
        json!({"data":{"drives":[{"id":9,"name":"Team Ensemble","account_id":1480441,"role":"user","in_maintenance":false,"quota":{"used":1}}],"users":[{"id":1}]}}),
    );
    let drives = h.call("kdrive_list_drives", json!({})).await;
    assert_eq!(drives["result"]["isError"], false, "{drives}");
    assert_eq!(
        result(&drives)["data"],
        json!([{"id":9,"name":"Team Ensemble","account_id":1480441,"role":"user","in_maintenance":false,"source_url":"https://kdrive.infomaniak.com/app/drive/9/files/1"}])
    );
    h.mock.lock().unwrap().response_statuses.insert(
        "GET /3/drive/77/files/1/files".into(),
        StatusCode::UNAUTHORIZED,
    );
    let wrong = h.call("kdrive_list", json!({"drive_id":77})).await;
    assert_eq!(
        wrong["result"]["_meta"]["error_code"], "drive_not_accessible",
        "{wrong}"
    );
    let health: String = h
        .app
        .store
        .with(|c| {
            Ok(c.query_row(
                "SELECT health FROM connections WHERE user_id=?",
                [h.user.id],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(health, "connected");
    h.mock
        .lock()
        .unwrap()
        .response_statuses
        .insert("GET /2/profile".into(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        h.call("kdrive_list", json!({"drive_id":77})).await["error"],
        "reconnection_required"
    );
    for invalid in [json!({"data":{}}), json!({"data":{"drives":[{"id":"9"}]}})] {
        h.mock
            .lock()
            .unwrap()
            .responses
            .insert("GET /2/drive/init?with=drives".into(), invalid);
        h.mock.lock().unwrap().response_statuses.clear();
        let out = h.call("kdrive_list_drives", json!({})).await;
        assert_eq!(
            out["result"]["_meta"]["error_code"], "provider_error",
            "{out}"
        );
    }
}
