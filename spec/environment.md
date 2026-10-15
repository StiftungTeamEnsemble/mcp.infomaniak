# Environment (.env)

Server-side bootstrap. The allowed domain is confirmed; other values illustrate deployment.

```dotenv
PUBLIC_URL=https://mcp.example.org              # Replace with chosen HTTPS origin
SQLITE_PATH=/data/mcp.sqlite                   # Persistent database
CREDENTIAL_ENCRYPTION_KEY=<base64-32-byte-key> # Secret; stored outside SQLite
INFOMANIAK_CLIENT_ID=<client-id>               # One organisation OAuth/OIDC application
INFOMANIAK_CLIENT_SECRET=<client-secret>       # Configure once on the server
ALLOWED_EMAIL_DOMAINS=team-ensemble.ch          # Comma-separated login domains
```

- Replace placeholders; reject missing/invalid values at startup.
- Keep `.env` out of Git/frontend bundles; retain the encryption key across redeployments.
- Register the exact `PUBLIC_URL` + `/auth/infomaniak/callback` URL in the [Infomaniak application setup](identity.md#before-implementation).

## Domain restriction

- Require a nonempty allowlist; match canonical email domains exactly, ignoring case. Subdomains require explicit entries.
- Require email from validated Infomaniak OIDC with `email_verified=true`; reject missing/unverified emails and client-supplied identity claims.
- Enforce at sign-in, account linking and every request, including existing sessions after allowlist changes.
- Domain matching alone establishes neither identity nor permissions; admission follows [onboarding policy](identity.md#onboarding) and [access rules](access.md).
- Restrict user identities, not mail recipients, invitees or shared-resource domains.
