# Agent instructions

- Read the [spec index](spec/README.md) and [development context](spec/development.md), including [local integration testing](spec/development.md#local-chatgpt-testing), before work.
- Follow the [coding and implementation principles](spec/coding-style.md) for security, reliability, scope, logging and reuse.
- Follow the [module contract](spec/module-system.md) when adding or changing services.
- Keep module tool names, arguments and results explicit; link provider adapters for upstream API calls.
- Keep deployment variables and domain restrictions in the [environment spec](spec/environment.md). Keep `compose.yaml` platform-portable (Coolify uses it alone); local-only settings belong in `compose.override.yaml`.
- Preserve [browser-based sign-in](spec/identity.md) when changing authentication. Service access uses guided [personal API tokens](spec/identity.md#infomaniak-authorisation) that must match the signed-in account; store one credential per user and provider (all Infomaniak services share `infomaniak`) and keep that setup simple.
- Change existing databases only through a new step in `src/migrations.rs` (SQLite `user_version`); keep `src/schema.sql` the current fresh-database schema.
- Keep [verification status](spec/development.md#verification-status) current; provider mocks never satisfy the two-employee or real-client integration prerequisites.
- Verify continuation through actual tool/resource inputs, including schema bounds, and recheck the exact client token or browser session after asynchronous credential renewal; cover both with integration regressions in `test/`.
- Preserve [transport authorisation errors and explicit mutation outcomes](spec/mcp.md#behaviour-and-results); record safe recovery context before every module write, including across tool deadlines. Never hide known provider limitations behind successful results.
- Apply [newly validated identity claims](spec/environment.md#domain-restriction) to existing access checks, including after credential renewal.
- Validate provider-returned identifiers before constructing paths and reused mail header/attachment metadata before uploads; preserve authorisation failures from sending-identity lookup. Check [Mail listing/message/draft identities](spec/modules/mail.md#behaviour) against the returned/requested objects and mailbox; never infer global-search folders from the query base. Retain safe returned draft IDs for uncertain mutations.
- Check [kDrive result identities](spec/modules/kdrive.md#behaviour) against the requested file/drive before downloads or discovery caching; validate file-mutation acknowledgements even when IDs are omitted and retain safe returned IDs for recovery.
- Calendar updates send only the reference's writable fields. Validate [Calendar result identities](spec/modules/calendar.md#behaviour), including deletion acknowledgements, and reject incomplete lists; preserve known event IDs when a mutation result is uncertain.
- [kChat](spec/modules/kchat.md) requires `KCHAT_TEAM`; validate returned team/channel/thread/post/user identities and record phase before each write, including the direct-channel step.
- Production host (Coolify): `ssh -p 22 -i ${HOME}/.ssh/id_ed25519 root@91.98.192.176`; SQLite is in `/DATA/mcp/infomaniak`. Never print, copy off-host or log stored credentials.
- Keep this file concise: agent instructions and links. Document project facts once in `/spec`.

## Documentation

- Keep `/spec` limited to required behaviour and important project-specific context: architecture, decisions, setup/build/test commands, structure, conventions, workflows, dependencies and gotchas.
- Assume a top-class implementation team. State outcomes and constraints; leave routine implementation decisions to the team.
- Define shared behaviour once and link to it. Avoid repetition, obvious defaults and implementation walkthroughs.
- Keep deployment guidance portable; avoid hosting-platform assumptions.
- Include edge cases only when they clarify meaningful product decisions.
- Reduce wording without losing requirements. Distinguish planned behaviour from implemented capabilities; record only verified facts and commands.

## Required maintenance

- Every agent must update `AGENTS.md` whenever meaningful changes are made, assumptions are clarified, or new project context is discovered.
- Update affected `/spec` documents and links in the same task. Preserve useful content; remove or correct stale information.
