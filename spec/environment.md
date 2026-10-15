# Environment (.env)

Server-side deployment bootstrap; values below are examples, not defaults.

```dotenv
PUBLIC_URL=https://mcp.example.org              # Public HTTPS origin
SQLITE_PATH=/data/mcp.sqlite                   # Persistent database
CREDENTIAL_ENCRYPTION_KEY=<base64-32-byte-key> # Secret; stored outside SQLite
ALLOWED_EMAIL_DOMAINS=example.org              # Comma-separated login domains
```

- Replace placeholders; reject missing/invalid values at startup.
- Keep `.env` out of Git/frontend bundles; retain the encryption key across redeployments.
- Integration-specific variables follow the [validated identity setup](identity.md#before-implementation).

## Domain restriction

- Require a nonempty allowlist; match canonical email domains exactly, ignoring case. Subdomains require explicit entries.
- Use authoritative email from the trusted IdP, never client-supplied addresses; reject missing/untrusted identity emails.
- Enforce at sign-in, account linking and every request, including existing sessions after allowlist changes.
- Domain approval does not provision users, grant permissions or establish account ownership; [access rules](access.md) still apply.
- Restrict user identities, not mail recipients, invitees or shared-resource domains.
