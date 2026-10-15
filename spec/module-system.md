# Module system

- Mail, Calendar, kDrive and future services implement the same registration contract within the shared Rust server.
- Keep provider-specific behaviour inside each module. Adding or changing a module requires no edits to unrelated modules or shared authentication/admin infrastructure.

## Registration contract

Each module registers with the kernel:

- Stable module ID, display name and namespace.
- MCP tools/resources and handlers; module-owned HTTP routes where needed.
- Required API scopes, credentials, configuration and dependencies.
- Permissions required by each operation.
- Admin options/configuration fields and health/status reporting.

## Shared kernel

- Compose registered tools, resources and routes; reject conflicting IDs, namespaces or routes before activation.
- Apply shared [identity](identity.md), [access](access.md), [SQLite storage](architecture.md) and [logging](coding-style.md#logging) rules to every handler. Registration never grants user access.
- Derive common admin entries and settings from registration metadata.
- Disabled or unconfigured modules expose no data operations; administrators can still configure them and inspect status.
- A module outage must not disable unrelated modules.

## Reference implementations

Use these capability references within the shared, per-user architecture:

- [Infomaniak Calendar MCP](https://github.com/Infomaniak/mcp-server-calendar)
- [Infomaniak kDrive MCP](https://github.com/Infomaniak/mcp-server-kdrive)
- [Infomaniak Mail MCP](https://github.com/Infomaniak/mcp-server-mail)
