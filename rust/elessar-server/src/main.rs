//! `elessar-server [--scenario id] [--scenarios-dir dir] [--host h] [--port 8778] [--check]`

use elessar_server::app::App;
use elessar_server::config::{self, Config};
use elessar_server::{packcheck, routes};
use std::process::ExitCode;
use std::sync::Arc;

async fn serve(app: Arc<App>, host: &str, port: u16) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind((host, port)).await.map_err(|e| format!("cannot listen on {host}:{port}: {e}"))?;
    let bound = listener.local_addr().map_err(|e| e.to_string())?;
    app.set_base_url(format!("http://{host}:{}", bound.port()));
    let mut packs: Vec<&str> = app.packs.keys().map(String::as_str).collect();
    packs.sort_unstable();
    println!("ELESSAR on {}  (engine: rust; scenario: {}; packs: {})", app.base_url(), app.default, packs.join(", "));
    let off = "off (no OPENROUTER_API_KEY)";
    println!("  decision model (Jev): {}", if app.jev.available() { &app.jev.model } else { off });
    println!("  inference model (agent): {}", if app.jev.available() { &app.config.inference_model } else { off });
    for (name, errors) in &app.failures {
        println!("  skipped invalid pack '{name}': {}", errors.first().map_or("", String::as_str));
    }
    axum::serve(listener, routes::router(app)).await.map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    let root = config::find_root();
    let args = match config::Args::parse(std::env::args().skip(1), &root) {
        Ok(args) => args,
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    if args.check {
        return if packcheck::print_check(&args.scenarios_dir) { ExitCode::SUCCESS } else { ExitCode::FAILURE };
    }
    // Before any thread exists: this changes the process environment.
    config::load_dotenv(&root.join(".env"));
    let config = Config::from_env(&root, &args);
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot start the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(async {
        let app = App::new(config)?;
        serve(app, &args.host, args.port).await
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
