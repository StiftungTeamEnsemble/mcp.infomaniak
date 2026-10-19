# Environment (.env)

Server-side bootstrap. The allowed domain is confirmed; other values illustrate deployment.

```dotenv
PUBLIC_URL=https://mcp.example.org              # Replace with chosen HTTPS origin
SQLITE_PATH=/data/mcp.sqlite                   # Persistent database
CREDENTIAL_ENCRYPTION_KEY=<base64-32-byte-key> # Secret; stored outside SQLite
INFOMANIAK_CLIENT_ID=<client-id>               # Organisation application for this environment
INFOMANIAK_CLIENT_SECRET=<client-secret>       # Configure once on the server
ALLOWED_EMAIL_DOMAINS=team-ensemble.ch          # Comma-separated login domains
BIND_ADDR=127.0.0.1:3000                       # Optional internal listener; Compose uses 0.0.0.0:3000
```

- Replace placeholders; reject missing/invalid values at startup.
- Keep `.env` out of Git/frontend bundles; retain the encryption key across redeployments.
- The native binary reads exported variables; `compose.yaml` interpolates the required variables from `.env` or the deployment platform (for example Coolify) and fails when one is missing. Generate the key with `cargo run -- generate-key`. `PUBLIC_URL` must be an HTTPS origin with no path, query, fragment or user information. TLS/proxy provisioning is external to the application; forwarded headers are ignored.
- Compose uses `/data/mcp.sqlite` in its named volume regardless of the native example's `SQLITE_PATH` value. Restrict the data directory and backups to the application/operator; the database file is created with mode `0600` on Unix.
- Register the exact `PUBLIC_URL` + `/auth/infomaniak/callback` URL in the [Infomaniak application setup](identity.md#before-implementation).
- For local integration testing, use the [development tunnel origin and isolated credentials](development.md#local-chatgpt-testing).

## Domain restriction

- Require a nonempty allowlist; match canonical email domains exactly, ignoring case. Subdomains require explicit entries.
- Require email from validated Infomaniak OIDC with `email_verified=true`; reject missing/unverified emails and client-supplied identity claims.
- Enforce at sign-in, account linking and every request, including existing sessions after allowlist changes. Apply an existing identity's latest validated email even when its new domain is rejected, including claims observed during token renewal.
- Domain matching alone establishes neither identity nor permissions; admission follows [onboarding policy](identity.md#onboarding) and [access rules](access.md).
- Restrict user identities, not mail recipients, invitees or shared-resource domains.
