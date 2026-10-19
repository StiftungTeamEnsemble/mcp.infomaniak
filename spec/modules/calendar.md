# Calendar

Implements the [module contract](../module-system.md) within the [shared MCP server](../README.md); all shared requirements apply.

Baseline: [tools](https://github.com/Infomaniak/mcp-server-calendar#tools), [schemas](https://github.com/Infomaniak/mcp-server-calendar/blob/main/src/index.ts) and [provider calls](https://github.com/Infomaniak/mcp-server-calendar/blob/main/src/calendar-client.ts), reviewed 2026-10-15. [Call notation](../mcp.md#calls).

## Tools

- `calendar_list_calendars()` → calendars with `id`, `name`.
- `calendar_list_events(from, to, calendar_id?)` → events within the interval.
- `calendar_get_event(event_id)` → full event.
- `calendar_create_event(title, start, end, description?, attendees?, rrule?, calendar_id?)` → created event; sends actionable attendee invitations.
- `calendar_update_event(event_id, title?, start?, end?, description?, attendees?, rrule?, calendar_id?, notify_attendees=false, recurrence_scope?)` → updated event.
- `calendar_delete_event(event_id, calendar_id?, notify_attendees=false, recurrence_scope?)` → deletion and cancellation-notification outcome.

## Behaviour

- `attendees` is a JSON-encoded email array; `rrule` follows RFC 5545; `rrule=""` removes recurrence.
- Offset-free inputs use the user's Infomaniak profile timezone; offset-bearing timestamps preserve their instant. Date-only means midnight; server timezone has no effect.
- Listing/creation default to the user's primary calendar; updates/deletion retain the event's actual calendar unless explicitly selected otherwise.
- Partial updates preserve omitted fields, including attendees, alarms, attachments and room data.
- Project extension: recurring updates/deletes require `recurrence_scope=series|occurrence`; reject unsupported occurrence operations.
- Return event identity/timezone and notification outcomes; list/get include `bookable_resource_name` when present.
- Validate event IDs and any returned calendar identity against the requested objects. Malformed creation/update results report uncertainty with known event/calendar IDs and mutation phase. Explicitly incomplete or oversized lists require a narrower interval rather than reporting complete results.
- Deletion acknowledgements must be objects and may omit identities; any returned event/calendar identity must match the requested objects. Malformed acknowledgements report uncertainty with known and safe returned event IDs.
