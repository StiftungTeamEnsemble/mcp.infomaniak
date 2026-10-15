# Coding and implementation principles

## Security and reliability

- Security is the highest priority; never weaken [access controls or user isolation](access.md) for convenience, speed or simplicity.
- Fail closed when identity or authorisation cannot be verified.
- Handle errors explicitly; bound external waits/retries and test security boundaries, failures and recovery.

## Scope and reuse

- Implement only required behaviour; prefer the smallest maintainable solution over speculative features or complexity.
- Reuse existing code and abstract repeated behaviour where it simplifies maintenance; avoid premature abstractions.

## Logging

- Persist structured operational and audit logs in server-side [SQLite storage](architecture.md), across restarts and deployments.
- Audit authentication, admin changes and tool execution. Include timestamp, severity, request correlation, actor/client, action, outcome and safe error details where applicable.
- Never log credentials, tokens or message/file bodies.
- Bound log growth through retention so logging cannot exhaust application storage.
