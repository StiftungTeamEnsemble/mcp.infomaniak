use ensemble_mcp::{
    App, Error, Result,
    config::Config,
    crypto,
    identity::Provider,
    store::{Store, now},
};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("generate-key") {
        use base64::{Engine, engine::general_purpose::STANDARD};
        println!("{}", STANDARD.encode(rand::random::<[u8; 32]>()));
        return Ok(());
    }
    let config = Config::from_env()?;
    let store = Store::open(&config.sqlite_path)?;
    if std::env::args().nth(1).as_deref() == Some("users") {
        store.with(|c| {
            let mut stmt = c.prepare("SELECT subject,email,status,admin FROM users ORDER BY id")?;
            for row in stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                ))
            })? {
                let (sub, email, status, admin) = row?;
                println!("{sub}\t{email}\t{status}\tadmin={admin}");
            }
            Ok(())
        })?;
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        let addr: std::net::SocketAddr = config
            .bind_addr
            .parse()
            .map_err(|_| Error::config("BIND_ADDR"))?;
        let ip = if addr.ip().is_unspecified() {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            addr.ip()
        };
        let addr = std::net::SocketAddr::new(ip, addr.port());
        let response = Provider::http_client()?
            .get(format!("http://{addr}/health"))
            .send()
            .await
            .map_err(|_| Error::internal())?;
        if !response.status().is_success() {
            return Err(Error::internal());
        }
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("admin") {
        let subject = std::env::args()
            .nth(2)
            .ok_or_else(|| Error::invalid("Usage: ensemble-mcp admin <Infomaniak subject>"))?;
        let user = store.with(|c| {
            let (user, email): (i64, String) = c.query_row(
                "SELECT id,email FROM users WHERE issuer=? AND subject=?",
                rusqlite::params![ensemble_mcp::identity::ISSUER, subject],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if !config.admits(&email) {
                return Err(Error::denied());
            }
            c.execute(
                "UPDATE users SET admin=1,status='active' WHERE id=?",
                [user],
            )?;
            Ok(user)
        })?;
        store.audit("cli", Some(user), None, "admin.bootstrap", "ok")?;
        println!("Administrator assigned to local user {user}");
        return Ok(());
    }
    if !std::path::Path::new("web/dist/app.js").is_file() {
        return Err(Error::config(
            "Build the interface first: npm ci && npm run build",
        ));
    }
    let provider = Provider::discover().await?;
    let addr = config.bind_addr.clone();
    let app = App::new(config, store, provider)?;
    app.store.prune()?;
    app.store
        .audit(&crypto::random(), None, None, "server.start", "ok")?;
    let upkeep = app.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
        loop {
            interval.tick().await;
            if upkeep.store.prune().is_err() {
                eprintln!("Storage maintenance failed at {}", now());
            }
        }
    });
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|_| Error::config("Cannot bind BIND_ADDR"))?;
    eprintln!(
        "ensemble-mcp listening on {addr}; HTTPS origin {}",
        app.config.public_url
    );
    axum::serve(listener, ensemble_mcp::web::router(app))
        .with_graceful_shutdown(shutdown())
        .await
        .map_err(|_| Error::internal())
}

async fn shutdown() {
    let interrupt = tokio::signal::ctrl_c();
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => eprintln!("Could not install the termination signal handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {result=interrupt=>{if result.is_err(){eprintln!("Could not install the interrupt signal handler");}},_=terminate=>{}}
}
