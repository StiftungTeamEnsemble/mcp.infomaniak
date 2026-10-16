# Agent instructions

- Read the [spec index](spec/README.md) and [development context](spec/development.md), including [local integration testing](spec/development.md#local-chatgpt-testing), before work.
- Follow the [coding and implementation principles](spec/coding-style.md) for security, reliability, scope, logging and reuse.
- Follow the [module contract](spec/module-system.md) when adding or changing services.
- Keep module tool names, arguments and results explicit; link provider adapters for upstream API calls.
- Keep deployment variables and domain restrictions in the [environment spec](spec/environment.md).
- Preserve [browser-based onboarding](spec/identity.md) when changing authentication; employees must never configure API keys.
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
