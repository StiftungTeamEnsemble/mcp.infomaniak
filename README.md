# Team Ensemble MCP

Organisation-operated Rust service for Infomaniak Mail, Calendar, kDrive and kChat, with a React connection and administration portal. Requirements are in [spec](spec/README.md).

Compatible MCP Apps clients can show shared calendar availability, individual kChat/email messages and expandable kDrive folders inline. Use the four focused [app tools](spec/mcp.md#mcp-apps); existing data tools remain available for ordinary queries. Refresh client connections after deploying tool or template changes.

## Run locally

Requires Rust 1.97+, Node.js 24+ and an organisation Infomaniak OAuth/OIDC application. Use a public HTTPS origin, including for local browser testing.

```sh
npm ci
npm run build
cp .env.example .env
cargo run -- generate-key
```

Fill `.env` with the generated key, public origin, SQLite path, application client ID/secret and allowed domains. See the [environment contract](spec/environment.md). The native binary reads exported environment variables:

```sh
set -a
. ./.env
set +a
cargo run --locked
```

Open `PUBLIC_URL` through your HTTPS reverse proxy or [development tunnel](spec/development.md#local-chatgpt-testing). Sign in, then explicitly assign the first administrator from the server:

```sh
cargo run -- users
cargo run -- admin '<exact Infomaniak subject from users>'
```

Services start disabled. Complete the [two-employee provider verification](spec/identity.md#before-implementation), record its evidence under Administration → Services, enable the verified services and set user/group grants. Employees connect by pasting a personal Infomaniak API token in the portal, which guides them through creating it. Add `PUBLIC_URL/mcp` as an OAuth MCP connection in ChatGPT or Claude and approve client-specific scopes.

To admit organisation members whose OIDC email-verification flag is false, configure [member admission](spec/identity.md#organisation-member-admission). The directory lookup reuses a designated administrator's saved key; employees still save and use their own API keys.

## Docker Compose

```sh
docker compose up --build -d
docker compose exec mcp ensemble-mcp users
docker compose exec mcp ensemble-mcp admin '<exact subject>'
```

`compose.yaml` passes the required `.env` variables explicitly, stores SQLite in the `mcp-data` volume and only exposes port 3000 to its network. Locally, Compose also merges `compose.override.yaml`, which binds `127.0.0.1:3000`. Put an HTTPS proxy in front of the whole application. Retain the volume and encryption key. Use one application instance per SQLite database; make consistent SQLite backups with the application stopped or SQLite's backup API.

## Coolify

1. Create the Infomaniak application and register `https://<domain>/auth/infomaniak/callback` ([prerequisites](spec/identity.md#before-implementation)). Generate the key locally with `cargo run -- generate-key` or `docker run --rm $(docker build -q .) generate-key`.
2. In Coolify, add a resource from this Git repository with the **Docker Compose** build pack and Docker Compose location `/compose.yaml`.
3. Set the `mcp` service domain to `https://<domain>:3000`; the port suffix tells Coolify's proxy the internal port.
4. Fill the required variables Coolify lists from `compose.yaml`: `PUBLIC_URL` (exactly `https://<domain>`), `CREDENTIAL_ENCRYPTION_KEY`, `INFOMANIAK_CLIENT_ID`, `INFOMANIAK_CLIENT_SECRET` and `ALLOWED_EMAIL_DOMAINS`; for kChat also set the optional `KCHAT_TEAM`. Keep them out of build variables.
5. Deploy and wait for the container healthcheck. Sign in at `PUBLIC_URL`, then assign the administrator from the service terminal: `ensemble-mcp users`, `ensemble-mcp admin '<exact subject>'`.
6. Back up the `mcp-data` volume; never change the encryption key after first start. If you replace the volume with a host bind mount, the directory must be writable by the container user: `chown 10001:10001 <dir> && chmod 700 <dir>`; otherwise startup fails with `internal_error`.

## Verify

```sh
cargo test --locked
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
npm run build
npx playwright install chromium
npm test
docker compose config --quiet  # requires a filled .env
docker build -t ensemble-mcp:local .
node test/container-smoke.mjs
```

With Google Chrome already installed, use `PLAYWRIGHT_CHANNEL=chrome npm test` instead of installing Chromium. Browser tests use a local fixture server; Rust integration tests run isolated HTTP provider mocks. The test signing key in `test/fixtures` is public test data. The container smoke test uses dummy credentials and temporary storage, requires public OIDC discovery/network access and checks startup, restart persistence and shutdown; it does not perform employee consent or authenticated provider operations.

See [verification status and remaining work](spec/development.md#verification-status). A passing mock suite is not proof of live Infomaniak or ChatGPT/Claude interoperability.
