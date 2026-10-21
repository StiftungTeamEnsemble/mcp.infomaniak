# Admin and user interface

Apply [access rules](access.md) throughout.

## Administrators

- Enable/disable services organisation-wide.
- Manage automatic/manual onboarding, pending approvals, local groups and default service grants.
- Grant per-service read/write access to users/groups.
- Show effective access, connection health and audit metadata; exclude content/secrets.
- Suspend users and revoke connections.

## Users

- Sign in with Infomaniak; show pending approval when manual onboarding applies.
- Shared endpoint and client connection instructions.
- Enabled services and client grants.
- Guided personal API token setup: link to the Infomaniak token page, the exact form values (name `MCP Server Team Ensemble`, application `Standardanwendung`, scopes `Alle`, validity `Unbegrenzt`, empty description) and a paste field. Once saved, the instructions collapse and can be reopened to replace the token. Users can disconnect their own token/clients.
