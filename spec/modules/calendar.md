# Calendar

Implements the [module contract](../module-system.md) within the [shared MCP server](../README.md); all shared requirements apply.

Baseline: [tools](https://github.com/Infomaniak/mcp-server-calendar#tools), [schemas](https://github.com/Infomaniak/mcp-server-calendar/blob/main/src/index.ts) and [provider calls](https://github.com/Infomaniak/mcp-server-calendar/blob/main/src/calendar-client.ts), reviewed 2026-10-15. [Call notation](../mcp.md#calls).

## Tools

- `calendar_list_calendars()` → calendars with `id`, `name`.
- `calendar_list_events(from, to, calendar_id?)` → events within the interval.
- `calendar_get_event(event_id)` → full event.
- `calendar_find_free_slots(calendar_ids[], from, to, duration_minutes=30, day_start="09:00", day_end="17:00", weekdays=[1,2,3,4,5])` → common free windows, per-calendar names and busy intervals, and working windows, with an [inline MCP App](../mcp.md#mcp-apps).
- `calendar_create_event(title, start, end, description?, attendees?, rrule?, calendar_id?)` → created event; sends actionable attendee invitations.
- `calendar_update_event(event_id, title?, start?, end?, description?, attendees?, rrule?, calendar_id?, notify_attendees=false, recurrence_scope?)` → updated event.
- `calendar_delete_event(event_id, calendar_id?, notify_attendees=false, recurrence_scope?)` → deletion and cancellation-notification outcome.

## Behaviour

- `attendees` is a JSON-encoded email array; `rrule` follows RFC 5545; `rrule=""` removes recurrence.
- Offset-free inputs use the user's Infomaniak profile timezone; offset-bearing timestamps preserve their instant. Date-only means midnight; server timezone has no effect.
- Listing/creation default to the user's primary calendar; updates/deletion retain the event's actual calendar unless explicitly selected otherwise.
- Partial updates preserve omitted fields, including attendees, alarms, attachments and room data. Updates send only the reference's writable fields; the provider rejects read-back metadata (HTTP 422). Unchanged offset-bearing times are sent as event-timezone wall time.
- Project extension: recurring updates/deletes require `recurrence_scope=series|occurrence`; reject unsupported occurrence operations.
- Return event identity/timezone and notification outcomes; list/get include `bookable_resource_name` when present. Room names are optional enrichment, as in the reference: a failed room lookup returns the events with null names and a `warning`; only reconnection failures stop the call.
- Validate event IDs and any returned calendar identity against the requested objects. Malformed creation/update results report uncertainty with known event/calendar IDs and mutation phase. Explicitly incomplete or oversized lists require a narrower interval rather than reporting complete results.
- Deletion acknowledgements must be objects and may omit identities; any returned event/calendar identity must match the requested objects. Malformed acknowledgements report uncertainty with known and safe returned event IDs.
- Availability compares 1–10 distinct accessible calendar IDs over at most 31×24 hours. Supply all relevant calendars for every participant; access is limited to the acting user's shared calendars. Working hours use the profile timezone, stay within one local day, and use ISO weekdays (Monday=1). Meeting duration is 5–1,440 minutes. All-day events block their returned start/end interval; explicitly `free` events do not block. Other events block conservatively. Merge overlapping/touching busy intervals and return gaps at least as long as the requested meeting.
- Availability fails if any list is inaccessible/incomplete or a busy event has invalid times or an unexpanded recurrence rule. Recurrence expansion remains a provider-validation prerequisite; never infer availability by ignoring recurrence. Existing repeated-hour listing limitations also apply. Return at most 100 windows; when `has_more`, continue with `context.next_from` as `from`, retaining the other inputs. Suggestions are neither reservations nor a guarantee of attendance.
