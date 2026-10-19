# Mail

Implements the [module contract](../module-system.md) within the [shared MCP server](../README.md); all shared requirements apply.

Baseline: [tools](https://github.com/Infomaniak/mcp-server-mail#tools), [schemas](https://github.com/Infomaniak/mcp-server-mail/blob/main/src/index.ts) and [provider calls](https://github.com/Infomaniak/mcp-server-mail/blob/main/src/mail-client.ts), reviewed 2026-10-15. [Call notation](../mcp.md#calls).

Except `mail_list_mailboxes`, every tool accepts `mailbox_uuid?`, defaulting to the user's primary mailbox; signatures omit it. IDs are strings.

## Mailboxes and reading

- `mail_list_mailboxes()` → `uuid`, `email`, `mailbox`, `is_primary`.
- `mail_list_folders()` → folder IDs, names, paths, roles and unread/total counts.
- `mail_create_folder(name, parent_folder_id?)` → created folder.
- `mail_rename_folder(folder_id, name)` → renamed folder.
- `mail_delete_folder(folder_id)` → deletion outcome.
- `mail_list_emails(folder_id, limit=50, offset=0, unseen?, flagged?)` → thread summaries; reject both filters being true.
- `mail_search_emails(query?, from?, to?, subject?, since?, before?, folder_id?, limit=50, offset=0)` → matching messages; require a search criterion; dates use `YYYY-MM-DD`; omitted folder searches all folders.
- `mail_read_email(folder_id, message_id)` → plain/HTML bodies, headers, participants, state and attachment metadata.
- `mail_download_attachment(folder_id, message_id, attachment_id)` → filename, MIME type, size and typed image/resource content.

## Sending and drafts

Compose inputs: `to`, `subject`, plain-text `body`; optional `cc`, `bcc`, `in_reply_to`, `in_reply_to_uid`, `references`, `from_email`, `from_name`. Recipients are comma-separated; sender identity must be permitted.

- `mail_send_email`: compose inputs plus `attachments[]?` → send outcome.
- `mail_create_draft`: compose inputs → `uuid`, `uid`, `resolved_from`, optional `warnings`.
- `mail_list_drafts(limit=50, offset=0)` → draft threads and identifiers.
- `mail_update_draft(draft_uuid, to?, subject?, body?, cc?, bcc?, attachments[]?)` → updated draft.
- `mail_send_draft(draft_uuid, delay=0)` → send/scheduled outcome; delay is seconds.
- `mail_delete_draft(draft_uuid)` → deletion outcome.

## Bulk actions

- `mail_mark_email(folder_id, message_ids[], read)` → read/unread outcome.
- `mail_flag_email(folder_id, message_ids[], flagged)` → star/unstar outcome.
- `mail_move_email(message_ids[], from_folder_id, to_folder_id)` → move outcome.
- `mail_archive_email(folder_id, message_ids[])` → archive outcome.
- `mail_delete_email(folder_id, message_ids[], permanent=false)` → Trash outcome; permanent deletion requires `true`.

## Behaviour

- Preserve mailbox/folder/message/thread identity. Thread summaries retain `thread_uid`, `first_message_uid`, folder identity and state. Bulk `message_ids[]` are nonempty, folder-scoped message UIDs.
- Listing identities must be usable and consistent across scoped UIDs, folder fields and any returned mailbox identity. Global-search results require their actual folder; the query's Inbox base is not a result identity. A draft thread without messages may retain a null first-message UID.
- Read/reply-source results must identify the requested message; validate any returned folder/mailbox identity before returning content or composing a reply. Accept numeric and folder-scoped provider UIDs without losing identity.
- Created folders require usable IDs; malformed responses report uncertainty with the parent-folder recovery context.
- Message searches, message lists and draft lists return `has_more` and `next_offset`; continue with the same filters and that `offset`. Page limits are 1–100; offsets are nonnegative safe JSON integers, and returned continuation offsets must be accepted unchanged.
- Reply formats: `in_reply_to` is a Message-ID; `in_reply_to_uid` is `UID@folder_id`; `references` contains space-separated Message-IDs.
- Attachments use [user-scoped transfers](../mcp.md), preserving filenames/MIME types.
- Validate all attachment content and header metadata before provider writes, including reused recipients, sender/reply headers and attachment metadata; reject header control characters while allowing multiline bodies. Validate provider identifiers before constructing paths; malformed upload identifiers report uncertainty. Failed multi-step sends/updates identify the known draft and completed attachment uploads.
- Draft editing never sends; accessible drafts survive reconnections/restarts. Omitted update fields retain their values.
- Draft reads require the requested UUID; validate any returned mailbox identity on reads, creation and mutations. Mutation acknowledgements must be objects and may omit UUIDs, but returned UUIDs must be usable and match the requested draft. Invalid mutation identities report uncertainty with known and safe returned draft IDs.
- Return effective sending identity, provider fallback warnings and actual send/scheduled outcome. Sending-identity lookup authorisation failures require reconnection or access correction before any write.
