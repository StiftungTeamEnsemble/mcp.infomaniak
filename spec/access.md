# Access and user isolation

- Each request acts as one active local user admitted through [onboarding](identity.md#onboarding) from an [allowed identity domain](environment.md#domain-restriction); pending/suspended users receive no access.
- Effective access = enabled services ∩ user/group policy ∩ connection-specific user consent/scopes ∩ current Infomaniak permissions.
- Shared resources require explicit Infomaniak grants.
- Filter tool/resource discovery; authorise every operation, including direct IDs, downloads and UI actions.
- Isolate credentials, sessions and caches per user; grants per connection. Supplied identities cannot select another user's credentials.
- Admin roles grant configuration access, never others' content; organisation credentials cannot replace user authorisation.
- Deactivation, suspension, service/policy changes and revoked consent apply before the next operation, including existing sessions/cached discovery.
