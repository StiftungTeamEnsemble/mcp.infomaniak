# Identity and connections

## SSO and provisioning

- SAML 2.0 user/admin SSO through the organisation's IdP; our authentication layer acts as service provider.
- SCIM 2.0 user/group provisioning, membership changes and deactivation from the same IdP.
- Stable SAML/SCIM identity mapping; email changes never relink accounts.

## Client authentication

- OAuth 2.1 authorisation-code flow, PKCE S256, discovery, compatible client registration and audience-bound tokens.
- Validate identity/scopes on every request.
- Follow [MCP](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization), [ChatGPT](https://developers.openai.com/plugins/build/auth) and [Claude](https://claude.com/docs/connectors/building) requirements.

## Infomaniak authorisation

- Use each user's authorised Infomaniak API credentials; verify account ownership. Support renewal, reconnection and revocation.
- Keep upstream credentials separate from MCP tokens and hidden from clients/tool results.
- SSO/SCIM grant no API access.

## Before implementation

- Confirm IdP/mappings: Infomaniak [receives SAML/SCIM identity/provisioning](https://www.infomaniak.com/en/support/faq/874/discover-infomaniaks-sso-connection); it is not established as our IdP/SCIM source.
- Validate application grants, renewal/revocation for `workspace:mail`, `workspace:calendar`, `user_info` and `drive`.
