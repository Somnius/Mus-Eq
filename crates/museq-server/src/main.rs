use museq_server::{api, config::Config, db};

#[tokio::main]
async fn main() {
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("museq-server: {e}");
            std::process::exit(2);
        }
    };
    let conn = db::open(&cfg.db).unwrap_or_else(|e| {
        eprintln!(
            "museq-server: cannot open database {}: {e}",
            cfg.db.display()
        );
        std::process::exit(2);
    });
    if let Some(seed) = &cfg.stations_seed {
        match std::fs::read_to_string(seed) {
            Ok(text) => match museq_server::stations::import(
                &conn,
                "trayfm",
                &museq_server::stations::parse_list(&text),
            ) {
                Ok(n) => println!(
                    "museq-server: {n} new stations imported from {}",
                    seed.display()
                ),
                Err(e) => eprintln!("museq-server: station import failed: {e}"),
            },
            Err(e) => eprintln!("museq-server: cannot read {}: {e}", seed.display()),
        }
    }
    let state = api::AppState::new(
        conn,
        cfg.library.clone(),
        cfg.token.clone(),
        cfg.skins_dir.clone(),
        cfg.web_dir.clone(),
    );
    let listener = tokio::net::TcpListener::bind(&cfg.bind)
        .await
        .unwrap_or_else(|e| panic!("cannot bind {}: {e}", cfg.bind));
    // Never print the token.
    println!(
        "museq-server listening on {} (library: {})",
        cfg.bind,
        cfg.library.display()
    );
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server error");
}
