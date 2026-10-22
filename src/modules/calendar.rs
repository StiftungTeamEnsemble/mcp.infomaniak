//! Provider adapter: https://github.com/Infomaniak/mcp-server-calendar/blob/main/src/calendar-client.ts
use super::*;
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use reqwest::Method;

fn result(value: Value, mut context: Value) -> Value {
    context["source_url"] = json!("https://ksuite.infomaniak.com/calendar");
    super::result(value, context)
}

pub fn module() -> Module {
    let fields = || {
        vec![
            ("title", id()),
            ("start", id()),
            ("end", id()),
            ("description", s()),
            ("attendees", s()),
            ("rrule", s()),
            ("calendar_id", path_id()),
        ]
    };
    let mut update = fields();
    update.extend([
        ("event_id", path_id()),
        ("notify_attendees", b()),
        ("recurrence_scope", choices(&["series", "occurrence"])),
    ]);
    let mut module = Module {
        id: "calendar",
        name: "Calendar",
        namespace: "calendar",
        provider: INFOMANIAK,
        scopes: vec!["workspace:calendar", "user_info"],
        routes: vec![],
        resources: None,
        handler: |c, n| Box::pin(call(c, n)),
        tools: vec![
            tool(
                "calendar_list_calendars",
                "List calendars granted by Infomaniak.",
                false,
                false,
                true,
                &[],
                vec![],
            ),
            tool(
                "calendar_list_events",
                "List events in a bounded interval; offset-free dates use your profile timezone.",
                false,
                false,
                true,
                &["from", "to"],
                vec![("from", id()), ("to", id()), ("calendar_id", path_id())],
            ),
            tool(
                "calendar_get_event",
                "Read an event with timezone and room metadata.",
                false,
                false,
                true,
                &["event_id"],
                vec![("event_id", path_id())],
            ),
            tool(
                "calendar_create_event",
                "Create an event and submit actionable attendee invitations.",
                true,
                false,
                true,
                &["title", "start", "end"],
                fields(),
            ),
            tool(
                "calendar_update_event",
                "Partially update an event, preserving omitted fields. Recurring events require explicit series scope; occurrences are unsupported.",
                true,
                true,
                true,
                &["event_id"],
                update,
            ),
            tool(
                "calendar_delete_event",
                "Delete from the event's actual calendar; optionally request attendee cancellation.",
                true,
                true,
                true,
                &["event_id"],
                vec![
                    ("event_id", path_id()),
                    ("calendar_id", path_id()),
                    ("notify_attendees", b()),
                    ("recurrence_scope", choices(&["series", "occurrence"])),
                ],
            ),
        ],
    };
    let event = json!({"type":"object","properties":{"id":{"type":["string","integer"]},"calendar_id":{"type":["string","integer"]},"title":{"type":"string"},"start":{"type":"string"},"end":{"type":"string"},"timezone_start":{"type":"string"},"timezone_end":{"type":"string"},"attendees":{"type":"array"},"alarms":{"type":"array"},"attachments":{"type":"array"},"rrule":{"type":["string","null"]},"bookable_resource_name":{"type":["string","null"]}}});
    for tool in &mut module.tools {
        tool.output = match tool.name.as_str() {
            "calendar_list_calendars" => {
                json!({"type":"object","properties":{"calendars":{"type":"array","items":{"type":"object","properties":{"id":{"type":["string","integer"]},"name":{"type":"string"},"default":{"type":"boolean"}}}}}})
            }
            "calendar_list_events" => json!({"type":"array","items":event}),
            _ => event.clone(),
        };
    }
    module
}

pub fn local_time(input: &str, timezone: &str) -> Result<(String, i64)> {
    let tz: Tz = timezone.parse().map_err(|_| {
        Error::new(
            502,
            "provider_error",
            "Infomaniak profile timezone is missing or invalid",
        )
    })?;
    if let Ok(instant) = DateTime::parse_from_rfc3339(input) {
        return Ok((
            instant
                .with_timezone(&tz)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            instant.timestamp(),
        ));
    }
    let iso = input.replace(' ', "T");
    let date = NaiveDateTime::parse_from_str(&iso, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(&iso, "%Y-%m-%dT%H:%M"))
        .or_else(|_| {
            NaiveDate::parse_from_str(&iso, "%Y-%m-%d").map(|d| d.and_hms_opt(0, 0, 0).unwrap())
        })
        .map_err(|_| Error::invalid("Use an ISO date or timestamp"))?;
    let instant = tz.from_local_datetime(&date).single().ok_or_else(|| {
        Error::invalid("Ambiguous or nonexistent local time; include a UTC offset")
    })?;
    Ok((
        date.format("%Y-%m-%d %H:%M:%S").to_string(),
        instant.timestamp(),
    ))
}

// Inputs always use the profile timezone. Retain the event's timezone where it
// can represent the instant unambiguously; use UTC during a repeated DST hour.
fn event_time(
    input: &str,
    profile_timezone: &str,
    event_timezone: &str,
) -> Result<(String, String, i64)> {
    let (_, timestamp) = local_time(input, profile_timezone)?;
    let timezone: Tz = event_timezone
        .parse()
        .map_err(|_| Error::new(502, "provider_error", "Event timezone is invalid"))?;
    let instant = timezone
        .timestamp_opt(timestamp, 0)
        .single()
        .ok_or_else(|| Error::invalid("Event timestamp is out of bounds"))?;
    if timezone
        .from_local_datetime(&instant.naive_local())
        .single()
        .is_none()
    {
        return Ok((
            instant
                .with_timezone(&chrono::Utc)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            "UTC".into(),
            timestamp,
        ));
    }
    Ok((
        instant.format("%Y-%m-%d %H:%M:%S").to_string(),
        event_timezone.into(),
        timestamp,
    ))
}

fn attendees(input: &str, profile: &Value) -> Result<Value> {
    let emails: Vec<String> = serde_json::from_str(input)
        .map_err(|_| Error::invalid("attendees must be a JSON-encoded email array"))?;
    if emails.len() > 100 || emails.iter().any(|e| !email(e)) {
        return Err(Error::invalid("Invalid attendees"));
    }
    if emails.is_empty() {
        return Ok(json!([]));
    }
    let organizer = profile["email"]
        .as_str()
        .filter(|s| email(s))
        .ok_or_else(|| Error::new(502, "provider_error", "Profile has no organizer email"))?;
    let mut people=emails.into_iter().filter(|e|!e.eq_ignore_ascii_case(organizer)).map(|e|json!({"className":"Attendee","address":e,"name":e,"state":"NEEDS-ACTION","organizer":false})).collect::<Vec<_>>();
    people.push(json!({"className":"Attendee","address":organizer,"name":profile["display_name"].as_str().unwrap_or(organizer),"state":"ACCEPTED","organizer":true}));
    Ok(json!(people))
}

/// Room names are optional enrichment, as in the reference client: a failed lookup
/// keeps the events and reports a warning; only an invalid token stops the call.
async fn annotate(c: &Call, mut events: Vec<Value>) -> Result<(Vec<Value>, Option<&'static str>)> {
    if !events.iter().any(|e| {
        e["bookable_resource_id"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    }) {
        return Ok((events, None));
    }
    let rooms = match c
        .app
        .provider
        .request(
            &c.token,
            Method::GET,
            format!(
                "{}/1/calendar/pim/bookable-resources/all",
                c.app.provider.api
            ),
            &[("with[]".into(), "permission".into())],
            None,
        )
        .await
    {
        Ok(rooms) => data(rooms),
        Err(e) if e.code == "reconnection_required" => return Err(e),
        Err(_) => {
            for event in &mut events {
                if event["bookable_resource_id"].is_string() {
                    event["bookable_resource_name"] = Value::Null;
                }
            }
            return Ok((events, Some("Room names unavailable from the provider")));
        }
    };
    for event in &mut events {
        if let Some(id) = event["bookable_resource_id"].as_str() {
            let name = rooms
                .as_array()
                .and_then(|rs| rs.iter().find(|r| r["uuid"] == id))
                .map(|r| r["name"].clone())
                .unwrap_or(Value::Null);
            event["bookable_resource_name"] = name;
        }
    }
    Ok((events, None))
}

fn provider_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|id| safe_provider_id(id) && id.len() <= 256)
        .map(String::from)
        .or_else(|| value.as_i64().filter(|id| *id > 0).map(|id| id.to_string()))
}

fn event_metadata(
    event: Value,
    expected_event: Option<&str>,
    expected_calendar: Option<&str>,
    mutation: bool,
    allow_acknowledgement: bool,
) -> Result<Value> {
    let id = provider_id(&event["id"]);
    if !event.is_object()
        || (id.is_none() && (!allow_acknowledgement || event.get("id").is_some()))
        || (id.is_some() && expected_event.is_some_and(|expected| id.as_deref() != Some(expected)))
        || event.get("calendar_id").is_some_and(|calendar| {
            let calendar = provider_id(calendar);
            calendar.is_none()
                || expected_calendar.is_some_and(|expected| calendar.as_deref() != Some(expected))
        })
    {
        let mut error = Error::new(
            502,
            if mutation {
                "mutation_uncertain"
            } else {
                "provider_error"
            },
            "Provider returned an invalid event or calendar identity; verify any mutation before repeating",
        );
        if mutation && let Some(id) = id {
            error = error.with_context(json!({"returned_event_id":id}));
        }
        return Err(error);
    }
    Ok(event)
}

/// The reference client's writable update fields. The provider rejects the
/// read-only metadata of a read-back event (HTTP 422 `validation_failed`).
fn writable_event(event: &Value, tz_start: &str, tz_end: &str) -> Value {
    let or = |field: &str, default: Value| match &event[field] {
        Value::Null => default,
        value => value.clone(),
    };
    // Read-back times may carry an offset; send event-timezone wall time.
    let time = |field: &str, tz: &str| match (event[field].as_str(), tz.parse::<Tz>()) {
        (Some(raw), Ok(tz)) => DateTime::parse_from_rfc3339(raw)
            .map(|t| json!(t.with_timezone(&tz).format("%Y-%m-%d %H:%M:%S").to_string()))
            .unwrap_or_else(|_| json!(raw)),
        _ => event[field].clone(),
    };
    let mut out = json!({
        "id": event["id"],
        "calendar_id": event["calendar_id"],
        "title": event["title"],
        "description": or("description", json!("")),
        "start": time("start", tz_start),
        "end": time("end", tz_end),
        "timezone_start": tz_start,
        "timezone_end": tz_end,
        "freebusy": or("freebusy", json!("busy")),
        "type": or("type", json!("event")),
        "fullday": or("fullday", json!(false)),
        "private": or("private", json!(false)),
        "color": event["color"],
        "attendees": or("attendees", json!([])),
        "alarms": or("alarms", json!([])),
        "attachments": or("attachments", json!([])),
        "rrule": or("rrule", json!("")),
        "meet_room_url": or("meet_room_url", json!("")),
        "notifyAttendees": event["notifyAttendees"],
        "parent_updated": false,
        "imip_request": false,
    });
    if event["location"].as_str().is_some_and(|l| !l.is_empty()) {
        out["location"] = event["location"].clone();
    }
    if let Some(room) = event.get("bookable_resource_id") {
        out["bookable_resource_id"] = room.clone();
    }
    out
}

pub async fn call(c: Call, name: String) -> Result<Value> {
    let args = &c.args;
    let base = format!("{}/1/calendar/pim", c.app.provider.api);
    if name == "calendar_list_calendars" {
        return Ok(result(
            data(
                c.app
                    .provider
                    .request(&c.token, Method::GET, format!("{base}/calendar"), &[], None)
                    .await?,
            ),
            json!({}),
        ));
    }
    let event_id = args["event_id"].as_str();
    let mut existing = if let Some(id) = event_id {
        event_metadata(
            data(
                c.app
                    .provider
                    .request(
                        &c.token,
                        Method::GET,
                        format!("{base}/event/{}", segment(id)),
                        &[],
                        None,
                    )
                    .await?,
            ),
            Some(id),
            None,
            false,
            false,
        )?
    } else {
        json!({})
    };
    if name == "calendar_get_event" {
        let (events, warning) = annotate(&c, vec![existing]).await?;
        return Ok(result(
            events[0].clone(),
            json!({"event_id":event_id,"warning":warning}),
        ));
    }
    if event_id.is_some() {
        let recurring = existing["rrule"].as_str().is_some_and(|s| !s.is_empty())
            || args["rrule"].as_str().is_some_and(|s| !s.is_empty())
            || existing["recurrence_id"]
                .as_str()
                .is_some_and(|s| !s.is_empty());
        if args["recurrence_scope"] == "occurrence" {
            return Err(Error::invalid(
                "Occurrence operations are not supported by this provider adapter",
            ));
        }
        if recurring && args["recurrence_scope"] != "series" {
            return Err(Error::invalid(
                "Recurring mutations require recurrence_scope=series",
            ));
        }
    }
    let calendar = if let Some(id) = args["calendar_id"].as_str() {
        json!(id)
    } else if event_id.is_some() {
        existing["calendar_id"].clone()
    } else {
        let calendars = data(
            c.app
                .provider
                .request(&c.token, Method::GET, format!("{base}/calendar"), &[], None)
                .await?,
        );
        calendars["calendars"]
            .as_array()
            .and_then(|cs| {
                cs.iter()
                    .find(|c| c["default"] == true)
                    .or_else(|| cs.first())
            })
            .map(|c| c["id"].clone())
            .ok_or_else(|| Error::new(502, "provider_error", "No primary calendar available"))?
    };
    let calendar_string = provider_id(&calendar).ok_or_else(|| {
        Error::new(
            502,
            "provider_error",
            "Provider returned an invalid calendar identity",
        )
    })?;
    let notify = args["notify_attendees"].as_bool().unwrap_or(false);
    if name == "calendar_delete_event" {
        c.set_mutation_context(json!({"event_id":event_id,"calendar_id":calendar,"phase":"delete_event","recovery":"Inspect the calendar before repeating"}))?;
        let response = c
            .app
            .provider
            .request(
                &c.token,
                Method::DELETE,
                format!("{base}/event/{}", segment(event_id.unwrap())),
                &[("calendar_id".into(), calendar_string.clone())],
                Some(json!({"notifyAttendees":notify,"imip_request":notify})),
            )
            .await?;
        return Ok(result(
            event_metadata(data(response), event_id, Some(&calendar_string), true, true)?,
            json!({"event_id":event_id,"calendar_id":calendar,"notifications_requested":notify,"notification_status":if notify {"provider_accepted"} else {"not_requested"}}),
        ));
    }
    let profile = data(
        c.app
            .provider
            .request(
                &c.token,
                Method::GET,
                format!("{}/2/profile", c.app.provider.api),
                &[],
                None,
            )
            .await?,
    );
    let timezone = profile
        .pointer("/preferences/timezone/name")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new(502, "provider_error", "Profile timezone is required"))?;
    if name == "calendar_list_events" {
        let (from, a) = local_time(text(args, "from")?, timezone)?;
        let (to, b) = local_time(text(args, "to")?, timezone)?;
        if b <= a || b - a > 366 * 86400 {
            return Err(Error::invalid(
                "List intervals must be positive and at most 366 days",
            ));
        }
        // The reference API uses profile-local wall-clock query bounds. Do not
        // silently lose an explicit offset when that wall time occurs twice.
        if local_time(&from, timezone).ok().map(|t| t.1) != Some(a)
            || local_time(&to, timezone).ok().map(|t| t.1) != Some(b)
        {
            return Err(Error::new(
                502,
                "provider_limitation",
                "Calendar listing bounds in a repeated local hour are unsupported; use bounds outside that hour",
            ));
        }
        let response = c
            .app
            .provider
            .request(
                &c.token,
                Method::GET,
                format!("{base}/event"),
                &[
                    ("calendar_id".into(), calendar_string.clone()),
                    ("from".into(), from),
                    ("to".into(), to),
                ],
                None,
            )
            .await?;
        if response["has_more"] == true {
            return Err(Error::new(
                502,
                "provider_limitation",
                "Provider returned an incomplete event list; use a smaller date interval",
            ));
        }
        let events = data(response)
            .as_array()
            .cloned()
            .ok_or_else(|| Error::new(502, "provider_error", "Invalid event list"))?;
        if events.len() > 500 {
            return Err(Error::invalid(
                "Too many events; use a smaller date interval",
            ));
        }
        let events = events
            .into_iter()
            .map(|event| event_metadata(event, None, Some(&calendar_string), false, false))
            .collect::<Result<Vec<_>>>()?;
        let (events, warning) = annotate(&c, events).await?;
        return Ok(result(
            json!(events),
            json!({"calendar_id":calendar,"timezone":timezone,"warning":warning}),
        ));
    }
    let create = name == "calendar_create_event";
    if create {
        existing = json!({"freebusy":"busy","type":"event","fullday":false,"timezone_start":timezone,"timezone_end":timezone,"attendees":[],"alarms":[],"attachments":[]});
    }
    let mut tz_start = existing["timezone_start"]
        .as_str()
        .unwrap_or(timezone)
        .to_owned();
    let mut tz_end = existing["timezone_end"]
        .as_str()
        .unwrap_or(timezone)
        .to_owned();
    let mut instants = [None, None];
    for (index, field, tz) in [(0, "start", &mut tz_start), (1, "end", &mut tz_end)] {
        if let Some(value) = args[field].as_str() {
            let (wall_time, target_timezone, timestamp) = event_time(value, timezone, tz)?;
            existing[field] = json!(wall_time);
            *tz = target_timezone;
            existing[format!("timezone_{field}")] = json!(tz);
            instants[index] = Some(timestamp);
        }
    }
    if instants.iter().any(Option::is_some) {
        for (index, field, tz) in [(0, "start", &tz_start), (1, "end", &tz_end)] {
            if instants[index].is_none() {
                instants[index] = Some(
                    local_time(
                        existing[field].as_str().ok_or_else(|| {
                            Error::new(502, "provider_error", "Event time is missing")
                        })?,
                        tz,
                    )?
                    .1,
                );
            }
        }
        if instants[1] <= instants[0] {
            return Err(Error::invalid("Event end must follow start"));
        }
    }
    for field in ["title", "description", "rrule"] {
        if let Some(v) = args.get(field) {
            existing[field] = v.clone();
        }
    }
    if let Some(att) = args["attendees"].as_str() {
        existing["attendees"] = attendees(att, &profile)?;
    }
    existing["calendar_id"] = calendar.clone();
    let notify = if create {
        existing["attendees"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    } else {
        notify
    };
    existing["notifyAttendees"] = json!(notify);
    existing["parent_updated"] = json!(false);
    existing["imip_request"] = json!(false);
    let url = event_id
        .map(|id| format!("{base}/event/{}", segment(id)))
        .unwrap_or_else(|| format!("{base}/event"));
    c.set_mutation_context(json!({"event_id":event_id,"calendar_id":calendar,"phase":if create {"create_event"} else {"update_event"},"recovery":"Inspect the calendar before repeating"}))?;
    let response = c
        .app
        .provider
        .request(
            &c.token,
            if create { Method::POST } else { Method::PUT },
            url,
            &[],
            Some(if create {
                existing
            } else {
                writable_event(&existing, &tz_start, &tz_end)
            }),
        )
        .await?;
    let event = event_metadata(
        data(response),
        event_id,
        Some(&calendar_string),
        true,
        false,
    )?;
    let id = provider_id(&event["id"]);
    Ok(result(
        event,
        json!({"event_id":id,"calendar_id":calendar,"timezone_start":tz_start,"timezone_end":tz_end,"notifications_requested":notify,"notification_status":if notify {"provider_accepted"} else {"not_requested"}}),
    ))
}
