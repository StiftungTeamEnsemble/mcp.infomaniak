# Architecture and deployment

- One organisation-operated Rust MCP server and HTTPS Streamable HTTP endpoint for all ChatGPT/Claude users.
- [Mail](modules/mail.md), [Calendar](modules/calendar.md) and [kDrive](modules/kdrive.md) register through the shared kernel's [module contract](module-system.md).
- Minimal React [interface](interface.md).
- Store all application data in SQLite; encrypt stored credentials. [Deployment bootstrap settings/secrets](environment.md) come from the environment.
- Portable Docker Compose deployment; persist the SQLite database across restarts and deployments.
