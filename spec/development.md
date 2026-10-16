# Development context

- Current state: specifications only; no application code, dependency manifests, container setup or test suite.
- [Architecture](architecture.md) describes the target stack and deployment, not implemented capabilities.
- Application setup/build/test commands are not yet defined. Record verified commands and dependencies here when introduced.
- [Shared specifications](README.md) define cross-module requirements; `spec/modules/` defines service capabilities; [AGENTS.md](../AGENTS.md) defines agent instructions.
- Before implementing identity, resolve the [provider prerequisites](identity.md#before-implementation).
- After documentation changes, verify local Markdown links and preserve existing requirements.

## Local ChatGPT testing

- Run the server locally; expose the whole application through a stable HTTPS tunnel. [sish](https://github.com/antoniomika/sish) supports SSH forwarding through a public gateway.
- Example: gateway `tunnel.example.org`, dev origin `https://mcp-dev.tunnel.example.org`, local server `127.0.0.1:3000`.
- Provision gateway DNS/TLS and authorised SSH keys; require the requested hostname instead of random assignment. See [sish configuration](https://docs.ssi.sh/advanced).

Example tunnel after gateway setup:

```sh
ssh -p 2222 -o ExitOnForwardFailure=yes \
  -R mcp-dev:80:127.0.0.1:3000 tunnel.example.org
```

- Set `PUBLIC_URL` to the dev HTTPS origin; forward `/mcp`, discovery, OAuth, callback and UI routes. Generate external URLs from `PUBLIC_URL`; trust forwarded headers only from configured proxies.
- Use separate development SQLite storage, encryption key and Infomaniak application credentials. Register `PUBLIC_URL` + `/auth/infomaniak/callback` with that application; ChatGPT's OAuth redirect remains a separate client registration.
- Preserve authentication headers and Streamable HTTP/SSE, including quiet intervals. Keep sish capture consoles disabled and redact OAuth queries/credentials from gateway logs.
- Keep normal authentication/domain checks; use manual approval and limited service grants for development testers.
- In [ChatGPT Plugins](https://chatgpt.com/plugins), choose **Add custom MCP server**, enter the dev HTTPS URL ending in `/mcp`, configure OAuth and connect. Workspace permissions apply. Follow [official connection steps](https://developers.openai.com/plugins/deploy/connect-chatgpt).
- Restart locally to test changes; refresh the ChatGPT connection after tool/schema/auth/UI changes, then retest. A hostname change also requires updated OAuth registrations and client reconnection.

[Secure MCP Tunnel](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels) is an alternative for supported OpenAI clients. It transports private MCP calls; the browser-facing OAuth server still needs a reachable endpoint.
