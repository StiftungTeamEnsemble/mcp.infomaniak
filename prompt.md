# Implement the MCP service

Read `spec/README.md` and every linked spec. Treat them as the requirements. Inspect existing work and continue from it.

Build a complete, usable service with minimal code, dependencies and abstractions. Keep the specified technology, behaviour and visual style. Resolve routine choices simply; report material contradictions before changing requirements.

We are in development: breaking data-architecture changes are allowed at any time; backward compatibility and data migrations are not required. Use the latest stable versions of all packages; verify versions when adding or updating them.

Repeat until complete:

1. Find the most important missing behaviour, defect or unnecessary complexity.
2. Make one coherent improvement; simplify as you go.
3. Verify with relevant tests in `test/`.
4. Review the whole application against the spec, then repeat.

Finish when requirements and tests pass and the service runs end to end. Provide startup instructions, required `.env` settings, verification results and any remaining limitations. Never claim unverified work is complete.

End your final response with exactly `CODEX_DONE` on its own line only when these criteria are verified and no required work remains; otherwise end with `CODEX_CONTINUE`. On later passes, review and verify again before declaring completion. Preserve the spec, tests, this prompt and the loop script.
