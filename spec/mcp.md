# MCP behaviour and object rendering

## Calls

- Shared Streamable HTTP endpoint: `POST /mcp`; discover tools with `tools/list`, invoke with `tools/call` using `name` and `arguments`. See the [tool protocol](https://modelcontextprotocol.io/specification/2025-11-25/server/tools).
- File resources use `resources/list`, `resources/templates/list` and `resources/read(uri)` through the same endpoint. See the [resource protocol](https://modelcontextprotocol.io/specification/2025-11-25/server/resources).
- In module signatures, `?` marks optional arguments, `=value` gives defaults, `[]` denotes arrays and `→` introduces results.

## Behaviour and results

- Namespaced tools; typed inputs/results; accurate read/write, destructive and external-effect annotations.
- Bounded lists/searches; continuation when incomplete.
- Distinct reconnection, access-denied and provider errors.
- Report partial/uncertain mutations; never silently repeat non-idempotent actions.
- Stable object IDs, meaningful summaries, source links where available; concise text alongside structured results.
- Binary content: typed MCP content/resources or authenticated user-scoped transfers; no arbitrary server paths.
- Mail/event/file cards with source navigation through supported MCP Apps, inspired by [Twenty](https://github.com/twentyhq/twenty/pull/26179). Follow [ChatGPT's UI contract](https://developers.openai.com/plugins/concepts/plugins#optional-ui).
- Complete functionality through text/structured results; all card actions follow [access rules](access.md). No separate chat application.
- Follow the shared [logging requirements](coding-style.md#logging).
