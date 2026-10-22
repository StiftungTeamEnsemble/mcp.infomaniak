use super::*;
use chrono::{Datelike, Duration, NaiveTime, Utc};

pub fn tool() -> Tool {
    let mut tool = super::tool(
        "calendar_find_free_slots",
        "Compare 1–10 accessible calendars and show common free windows inline. Supply every participant's relevant calendar IDs from calendar_list_calendars. Search at most 31 days; defaults to weekdays 09:00–17:00 in your profile timezone. Windows are suggestions, not reservations; inaccessible, incomplete or unexpanded recurring calendars fail explicitly. Continue with context.next_from as from and retain the other arguments.",
        false,
        false,
        true,
        &["calendar_ids", "from", "to"],
        vec![
            (
                "calendar_ids",
                json!({"type":"array","minItems":1,"maxItems":10,"items":path_id()}),
            ),
            ("from", id()),
            ("to", id()),
            (
                "duration_minutes",
                json!({"type":"integer","minimum":5,"maximum":1440,"default":30}),
            ),
            (
                "day_start",
                json!({"type":"string","minLength":5,"maxLength":5,"default":"09:00"}),
            ),
            (
                "day_end",
                json!({"type":"string","minLength":5,"maxLength":5,"default":"17:00"}),
            ),
            (
                "weekdays",
                json!({"type":"array","minItems":1,"maxItems":7,"items":{"type":"integer","minimum":1,"maximum":7},"description":"ISO weekdays, Monday=1; default Monday–Friday"}),
            ),
        ],
    );
    let interval = json!({"type":"object","properties":{"start":{"type":"string"},"end":{"type":"string"},"duration_minutes":{"type":"integer"}},"required":["start","end"]});
    tool.output = json!({"type":"object","properties":{
        "timezone":{"type":"string"},"duration_minutes":{"type":"integer"},
        "from":{"type":"string"},"to":{"type":"string"},
        "slots":{"type":"array","items":interval},
        "working_windows":{"type":"array","items":interval},
        "calendars":{"type":"array","items":{"type":"object","properties":{"calendar_id":{"type":"string"},"name":{"type":["string","null"]},"busy":{"type":"array","items":interval}},"required":["calendar_id","busy"]}}
    },"required":["timezone","duration_minutes","from","to","slots","working_windows","calendars"]});
    tool.ui = Some(crate::mcp_apps::Ui {
        uri: "ui://calendar/availability-v1.html",
        title: "Common availability",
        script: include_str!("app.js"),
    });
    tool
}

fn provider_error(message: &'static str) -> Error {
    Error::new(502, "provider_error", message)
}

fn merge(mut intervals: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    intervals.sort_unstable();
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (start, end) in intervals {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
            continue;
        }
        merged.push((start, end));
    }
    merged
}

fn timestamp(value: &Value, key: &str, tz_key: &str, timezone: &str) -> Result<i64> {
    let time = value[key]
        .as_str()
        .ok_or_else(|| provider_error("Event has no usable time"))?;
    local_time(time, value[tz_key].as_str().unwrap_or(timezone))
        .map(|t| t.1)
        .map_err(|_| {
            provider_error("Event time is invalid or ambiguous; availability cannot be determined")
        })
}

fn iso(timestamp: i64, timezone: Tz) -> String {
    // All timestamps originate in chrono's supported date range.
    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .unwrap()
        .with_timezone(&timezone)
        .to_rfc3339()
}

fn interval(start: i64, end: i64, timezone: Tz) -> Value {
    json!({"start":iso(start, timezone),"end":iso(end, timezone),"duration_minutes":(end-start)/60})
}

pub async fn find(c: Call) -> Result<Value> {
    let args = &c.args;
    let ids = args["calendar_ids"]
        .as_array()
        .ok_or_else(|| Error::invalid("Calendar IDs are required"))?;
    let mut unique = HashSet::new();
    for id in ids {
        if !unique.insert(
            id.as_str()
                .ok_or_else(|| Error::invalid("Invalid calendar ID"))?,
        ) {
            return Err(Error::invalid("Calendar IDs must be distinct"));
        }
    }
    let start_time =
        NaiveTime::parse_from_str(args["day_start"].as_str().unwrap_or("09:00"), "%H:%M")
            .map_err(|_| Error::invalid("day_start must use HH:MM"))?;
    let end_time = NaiveTime::parse_from_str(args["day_end"].as_str().unwrap_or("17:00"), "%H:%M")
        .map_err(|_| Error::invalid("day_end must use HH:MM"))?;
    if end_time <= start_time {
        return Err(Error::invalid(
            "day_end must be after day_start on the same day",
        ));
    }
    let duration = numeric(args, "duration_minutes", 30) * 60;
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
        .ok_or_else(|| provider_error("Profile timezone is required"))?;
    let tz: Tz = timezone
        .parse()
        .map_err(|_| provider_error("Profile timezone is invalid"))?;
    let (_, from) = local_time(text(args, "from")?, timezone)?;
    let (_, to) = local_time(text(args, "to")?, timezone)?;
    if to <= from || to - from > 31 * 86400 {
        return Err(Error::invalid(
            "Availability intervals must be positive and at most 31 days",
        ));
    }

    // Names are optional enrichment, as with room names: a failed lookup leaves
    // them null; only an invalid token stops the call.
    let names = match c
        .app
        .provider
        .request(
            &c.token,
            Method::GET,
            format!("{}/1/calendar/pim/calendar", c.app.provider.api),
            &[],
            None,
        )
        .await
    {
        Ok(list) => data(list)["calendars"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        Err(e) if e.code == "reconnection_required" => return Err(e),
        Err(_) => vec![],
    };
    let name = |id: &Value| {
        names
            .iter()
            .find(|calendar| provider_id(&calendar["id"]).as_deref() == id.as_str())
            .and_then(|calendar| calendar["name"].as_str())
            .map_or(Value::Null, |name| json!(name))
    };
    let mut all_busy = vec![];
    let mut calendars = vec![];
    for id in ids {
        // Reuse listing's identity, completeness, timezone and transport checks.
        let listed = Box::pin(super::call(
            Call {
                app: c.app.clone(),
                actor: c.actor.clone(),
                token: c.token.clone(),
                args: json!({"calendar_id":id,"from":args["from"],"to":args["to"]}),
                mutation_context: c.mutation_context.clone(),
            },
            "calendar_list_events".into(),
        ))
        .await?;
        let events = listed["data"]
            .as_array()
            .ok_or_else(|| provider_error("Invalid calendar list"))?;
        let mut busy = vec![];
        for event in events {
            if event["freebusy"] == "free" {
                continue;
            }
            if event["rrule"].as_str().is_some_and(|rule| !rule.is_empty()) {
                return Err(Error::new(
                    502,
                    "provider_limitation",
                    "Availability requires expanded recurring occurrences; this calendar returned a recurrence rule",
                ));
            }
            let start = timestamp(event, "start", "timezone_start", timezone)?;
            let end = timestamp(event, "end", "timezone_end", timezone)?;
            if end <= start {
                return Err(provider_error(
                    "Event interval is invalid; availability cannot be determined",
                ));
            }
            let (start, end) = (start.max(from), end.min(to));
            if end > start {
                busy.push((start, end));
            }
        }
        let busy = merge(busy);
        all_busy.extend_from_slice(&busy);
        calendars.push(json!({"calendar_id":id,"name":name(id),"busy":busy.into_iter().map(|(a,b)| interval(a,b,tz)).collect::<Vec<_>>()}));
    }
    let busy = merge(all_busy);
    let mut day = DateTime::<Utc>::from_timestamp(from, 0)
        .unwrap()
        .with_timezone(&tz)
        .date_naive();
    let last = DateTime::<Utc>::from_timestamp(to - 1, 0)
        .unwrap()
        .with_timezone(&tz)
        .date_naive();
    let weekdays = args["weekdays"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![json!(1), json!(2), json!(3), json!(4), json!(5)]);
    let mut slots = vec![];
    let mut working_windows = vec![];
    let mut next_from = Value::Null;
    while day <= last {
        if weekdays.contains(&json!(day.weekday().number_from_monday())) {
            let (_, begin) = local_time(&day.and_time(start_time).to_string(), timezone)?;
            let (_, finish) = local_time(&day.and_time(end_time).to_string(), timezone)?;
            let mut cursor = begin.max(from);
            let finish = finish.min(to);
            if finish > cursor {
                working_windows.push(interval(cursor, finish, tz));
            }
            for &(a, b) in &busy {
                if b <= cursor {
                    continue;
                }
                if a >= finish {
                    break;
                }
                if a - cursor >= duration {
                    slots.push(interval(cursor, a, tz));
                }
                cursor = cursor.max(b);
            }
            if finish - cursor >= duration {
                slots.push(interval(cursor, finish, tz));
            }
        }
        day = day
            .checked_add_signed(Duration::days(1))
            .ok_or_else(|| Error::invalid("Date is out of bounds"))?;
    }
    if slots.len() > 100 {
        next_from = slots[100]["start"].clone();
        slots.truncate(100);
    }
    let mut out = result(
        json!({"timezone":timezone,"duration_minutes":duration/60,"from":iso(from,tz),"to":iso(to,tz),"slots":slots,"working_windows":working_windows,"calendars":calendars}),
        json!({"next_from":next_from,"availability":"Only the supplied calendars were compared; recheck before booking"}),
    );
    out["has_more"] = json!(!next_from.is_null());
    Ok(out)
}
