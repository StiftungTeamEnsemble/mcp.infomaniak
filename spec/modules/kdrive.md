# kDrive

Implements the [module contract](../module-system.md) within the [shared MCP server](../README.md); all shared requirements apply.

Baseline: [tools](https://github.com/Infomaniak/mcp-server-kdrive#tools), [schemas/resources](https://github.com/Infomaniak/mcp-server-kdrive/blob/main/src/index.ts) and [provider calls](https://github.com/Infomaniak/mcp-server-kdrive/blob/main/src/kdrive-client.ts), reviewed 2026-10-15. [Call notation](../mcp.md#calls).

## Tools

Every tool additionally requires numeric `drive_id` for a user-authorised drive; signatures omit it. Item/directory IDs are numeric.

- `kdrive_search(query, directory_id?, depth?)` → drive-wide or selected-folder matches; folder depth: `child|unlimited`.
- `kdrive_list(file_id=1, type?, cursor?)` → folder items, `has_more`, continuation `cursor`; type: `file|dir`.
- `kdrive_info(file_id)` → item metadata.
- `kdrive_recents(type?)` → recent items; type: `file|dir|vault`.
- `kdrive_shared_with_me(type?)` → shared items; same type filter.
- `kdrive_upload_file(file_name, text_content?, base64_content?, directory_id=1)` → uploaded metadata; exactly one content input, UTF-8 or binary.
- `kdrive_download_file(file_id)` → typed file content with filename/MIME type.
- `kdrive_create_folder(name, file_id=1)` → created metadata; `file_id` selects the parent.
- `kdrive_rename(file_id, name)` → updated metadata.
- `kdrive_move(file_id, dest_id)` → moved metadata.
- `kdrive_delete(file_id)` → moves item to trash.
- `kdrive_restore(file_id, destination_directory_id)` → restored metadata.
- `kdrive_share_link(file_id, right, password?, valid_until?, can_download?, can_edit?, can_request_access?)` → link details; rights: `inherit|password|public`, expiry is a Unix timestamp.
- `kdrive_share_access(file_id, emails[], right, message?)` → invitation outcomes; rights: `manage|read|write`.

## Resources

- Discover files through `resources/list` and `resources/templates/list`; read `kdrive://{drive_id}/{file_id}` through `resources/read`.
- Listings retain URI, filename, MIME type and continuation; reads return URI, MIME type and base64 `blob`.

## Behaviour

- Keep drive identity in every item reference; sharing cannot exceed rights the acting user may delegate.
- Return sharing recipients, rights, expiry and resulting link/invitation status.
- Metadata includes item type, size, location, modification time and source link where available.
