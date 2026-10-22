# kChat

Implements the [module contract](../module-system.md) within the [shared MCP server](../README.md); all shared requirements apply.

Baseline: [tools](https://github.com/Infomaniak/mcp-server-kchat#tools), [schemas](https://github.com/Infomaniak/mcp-server-kchat/blob/main/src/index.ts) and [provider calls](https://github.com/Infomaniak/mcp-server-kchat/blob/main/src/kchat-client.ts), reviewed 2026-10-22. [Call notation](../mcp.md#calls).

The organisation's team is set once with `KCHAT_TEAM` ([environment](../environment.md)); calls use `https://{team}.kchat.infomaniak.com/api/v4` with the employee's [personal API token](../identity.md#infomaniak-authorisation) (scope `kchat`). IDs are strings (alphanumeric or hyphenated UUIDs); writes act as the token owner. Posts include their permalink `https://{team}.kchat.infomaniak.com/{team}/pl/{post_id}`.

## Tools

- `kchat_list_channels(limit=100, page=0)` → public team channels.
- `kchat_list_my_channels()` → the user's channels, including private, direct and group channels (project extension: needed to address private conversations).
- `kchat_get_channel_history(channel_id, limit=10, page=0)` → posts, newest first.
- `kchat_get_thread_replies(thread_id)` → root post and replies, oldest first.
- `kchat_get_users(limit=100, page=0)` → user profiles.
- `kchat_get_user_profile(user_id)` → profile.
- `kchat_post_message(channel_id, text)` → created post.
- `kchat_reply_to_thread(channel_id, thread_id, text)` → created reply.
- `kchat_add_reaction(post_id, reaction)` → reaction; emoji name without colons.
- `kchat_send_direct_message(username, text)` → post in the direct channel.

## Behaviour

- Without `KCHAT_TEAM`, tools fail with `module_unconfigured`; keep the module disabled until it is set.
- `limit` is 1–100. Paged results return `has_more` and `context.next_page`; repeat with the same arguments and that `page`. Lists the provider reports as incomplete, and unpaged lists above 1,000 items, fail explicitly.
- Validate provider IDs before reuse; returned team, channel, thread, post, user and reaction identities must match the request. Direct-message channels must be type `D`.
- Writes record channel/thread/post/user IDs and phase before each provider write. Direct messages open (or reuse) the direct channel, then post. Invalid post results report uncertainty with safe returned post IDs.
- Text is up to 16,383 characters. Files, edits and deletion are out of scope.
