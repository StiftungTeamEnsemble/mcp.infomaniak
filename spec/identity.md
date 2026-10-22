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

- Decision 2026-10-21: Infomaniak OAuth applications only grant sign-in scopes (`openid profile email phone`); API scopes return `invalid_scope` ([developer docs](https://developer.infomaniak.com/getting-started) document API access through personal access tokens). Service access therefore uses one personal API token per employee.
- Employees create the token in the [Infomaniak Manager](https://manager.infomaniak.com/v3/ng/accounts/token/list) and paste it once in the portal, which links there and shows the exact [form values](interface.md#users). They never create API applications.
- Accept a token only when `GET /2/profile` succeeds and returns the signed-in user's email (and subject, when an ID is returned); never accept a colleague's token. Store one encrypted credential per user and provider; the `infomaniak` credential serves every enabled Infomaniak service the user is granted, including services granted later. Other providers' modules never receive it. Its scopes are chosen in the Manager and enforced upstream. Provider 401s mark the connection for replacement; tokens have no server-side renewal.
- Disconnect removes the user's credential for all of that provider's services; the employee deletes the token in Infomaniak to invalidate it upstream.
- Keep upstream credentials separate from MCP tokens and hidden from clients/tool results.
- Signing in does not itself grant Mail, Calendar, kDrive or kChat API access.
- The delegated OAuth connect flow remains implemented for providers that grant API scopes, but the portal does not offer it. It requests every Infomaniak service's scopes and stores the same `infomaniak` credential; a service whose scopes were not granted requires reconnection.

## Before implementation

- Choose the public HTTPS origin. The organisation administrator creates an Infomaniak OAuth/OIDC application supporting server-side authorisation code; register `/auth/infomaniak/callback` under that origin and configure its [client credentials](environment.md).
- Test two employees: validated identity claims, personal-token ownership, API isolation and token deletion. Verify personal tokens with scopes `workspace:mail`, `workspace:calendar`, `user_info`, `drive`, `kchat`.
- Enable modules only after this access is verified.
