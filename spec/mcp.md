# MCP behaviour and object rendering

## Calls

- Shared Streamable HTTP endpoint: `POST /mcp`; discover tools with `tools/list`, invoke with `tools/call` using `name` and `arguments`. See the [tool protocol](https://modelcontextprotocol.io/specification/2025-11-25/server/tools).
- File resources use `resources/list`, `resources/templates/list` and `resources/read(uri)` through the same endpoint. See the [resource protocol](https://modelcontextprotocol.io/specification/2025-11-25/server/resources).
- In module signatures, `?` marks optional arguments, `=value` gives defaults, `[]` denotes arrays and `→` introduces results.

## Behaviour and results

- Namespaced tools; typed inputs/results; accurate read/write, destructive and external-effect annotations.
- Bounded lists/searches; continuation when incomplete.
- Distinct reconnection, access-denied and provider errors.
- MCP authorisation failures use HTTP 401/403, including upstream reconnection failures (403). Missing client scopes return a Bearer `insufficient_scope` challenge naming the required scope and resource metadata; local policy denials and upstream reconnection failures do not request more client consent.
- Report partial/uncertain mutations with known object IDs, failed phase and progress in text and safe error metadata, including after operation deadlines. Authorisation failures retain that recovery context alongside their HTTP error. Provider rejections add `provider_status`, the provider's machine-readable `provider_code` and the `provider_endpoint` path with ID segments masked (never descriptions/bodies) to error context and the tool audit outcome. Never silently repeat non-idempotent actions. An unreadable or oversized response after a write is an uncertain outcome.
- Stable object IDs, meaningful summaries, source links where available; concise text alongside structured results.
- Binary content: typed MCP content/resources or authenticated user-scoped transfers; no arbitrary server paths.
- Follow the shared [logging requirements](coding-style.md#logging).

## MCP Apps

- Four focused tools render inline views: `calendar_find_free_slots`, `kchat_show_message`, `mail_show_email` and `kdrive_show_folder`. Existing data/mutation tools keep text and structured results without generic cards. Apps remain useful as structured results in clients without UI support.
- Modules register versioned `ui://{module}/…` resources through their tools. The kernel rejects conflicting/cross-module URIs, lists only permitted resources and checks current module/client access on every read. Templates contain static assets, never credentials or provider content.
- Use `_meta.ui.resourceUri` (visibility `model` and `app`) and `text/html;profile=mcp-app`, with `openai/outputTemplate` and `openai/widgetAccessible` as compatibility aliases. Views call only their own app tools. Follow the [MCP Apps standard](https://modelcontextprotocol.io/docs/extensions/apps) and [OpenAI UI contract](https://developers.openai.com/plugins/build/chatgpt-ui). Change the resource URI when changing a deployed view's HTML, JavaScript or CSS incompatibly; refresh client connections after deployment.
- The shared bridge requests MCP Apps `2026-01-26` and accepts the host's negotiated version; it implements initialization, host results, capabilities/theme, sizing, teardown and bounded request waits. Interactions call normal authenticated MCP tools through the host; no direct provider requests, external assets or additional browser permissions are required.
- Treat message/file content as untrusted. kChat uses text; Mail rebuilds an allowlist of inert HTML formatting and blocks active content, remote images and styles. HTTPS links open through the host. Provider content never enters executable HTML.
- Views show people, channels, calendars and folders by name, never raw IDs; names are optional enrichment (failed lookups fall back to neutral labels, invalid tokens still require reconnection). Sizes are human-readable; links use validated `source_url` values.
- Calendar selection updates conversation context with a suggested start; it does not book a meeting. kDrive expands/paginates folders on demand through `kdrive_show_folder`, preserving each branch's selectors and cursor. Failed reads retain the existing tree and expose the error.
