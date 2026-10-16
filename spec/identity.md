# Identity and connections

## Sign in with Infomaniak

- One organisation-managed application per environment; the administrator configures its client ID/secret once on the server. Employees use browser sign-in/consent.
- Users/admins authenticate directly through [Infomaniak OAuth2/OIDC](https://www.infomaniak.com/en/support/faq/2567/add-and-manage-infomaniak-api-applications).
- Use authorisation code + PKCE S256 with `openid profile email`; trust issuer `https://login.infomaniak.com` and its [discovery metadata](https://login.infomaniak.com/.well-known/openid-configuration).
- Validate OIDC responses; bind local accounts to `(iss, sub)`. Email changes never relink accounts.

## Onboarding

- Automatically create a regular local user after the first login passes the [verified-domain policy](environment.md#domain-restriction); apply administrator-defined default service grants.
- Support manual approval instead: eligible users remain pending until approved. Both modes use the same identity/domain checks.
- Administrator roles require explicit assignment; suspended identities remain suspended across subsequent logins.
- Manage groups and suspension locally. Staff offboarding uses admin suspension until an automatic provider lifecycle integration is verified.

## Client authentication

- OAuth 2.1 authorisation-code flow, PKCE S256, discovery, compatible client registration and audience-bound tokens.
- Authenticate through Infomaniak, then issue separate MCP tokens; validate identity/scopes on every request.
- Follow [MCP](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization), [ChatGPT](https://developers.openai.com/plugins/build/auth) and [Claude](https://claude.com/docs/connectors/building) requirements.

## Infomaniak authorisation

- Obtain each user's API tokens through delegated authorisation; verify account ownership and automate token exchange/renewal. Support reconnection and revocation.
- Employees never create API applications or generate, copy or paste personal API keys/tokens.
- Keep upstream credentials separate from MCP tokens and hidden from clients/tool results.
- Signing in does not itself grant Mail, Calendar or kDrive API access.

## Before implementation

- Choose the public HTTPS origin. The organisation administrator creates an Infomaniak OAuth/OIDC application supporting server-side authorisation code; register `/auth/infomaniak/callback` under that origin and configure its [client credentials](environment.md).
- Test two employees through the same application: validated identity claims, independent consent, API isolation and token renewal/revocation. Verify required service scopes: `workspace:mail`, `workspace:calendar`, `user_info`, `drive`.
- Enable modules only after delegated access is verified. Unsupported grants require a supported provider integration; personal-token entry is not an accepted fallback.
