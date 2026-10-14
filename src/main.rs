#[tokio::main]
async fn main() -> std::process::ExitCode {
    let result = async {
        let mut development_proxy = false;
        for argument in std::env::args().skip(1) {
            if argument != "--dev-proxy" || development_proxy {
                return Err("Usage: audioportal [--dev-proxy]".to_owned());
            }
            development_proxy = true;
        }
        let path = audioportal::settings::settings_path().map_err(|error| error.message)?;
        let app =
            audioportal::web::router(path, development_proxy).map_err(|error| error.message)?;
        let listener = audioportal::web::bind()
            .await
            .map_err(|error| error.message)?;
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .map_err(|error| format!("HTTP server failed: {error}"))
    }
    .await;
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            std::process::ExitCode::FAILURE
        }
    }
}
