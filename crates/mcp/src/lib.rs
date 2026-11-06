mod args;
mod install;
mod server;

use std::time::Duration;

use rmcp::ServiceExt;
use tracing_appender::non_blocking::WorkerGuard;

/// Serves the MCP protocol over stdio until the client disconnects. Stdout
/// carries only protocol frames; logs go to `mcp.log` in the state directory.
pub fn serve() -> anyhow::Result<()> {
    let _guard = init_logging();
    let state =
        server::State::from_env().inspect_err(|e| tracing::error!(error = %e, "startup"))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        tracing::info!(version = env!("CARGO_PKG_VERSION"), "serving on stdio");
        let service = server::Spacehog::new(state)
            .serve(rmcp::transport::stdio())
            .await?;
        let reason = service.waiting().await?;
        tracing::info!(?reason, "client disconnected");
        anyhow::Ok(())
    });
    // A `scan` call blocked in `wait` must not keep the process alive after the client has gone.
    runtime.shutdown_timeout(Duration::from_secs(1));
    result
}

fn init_logging() -> Option<WorkerGuard> {
    let dir = spacehog_core::paths::state_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("mcp.log"))
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(file);
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .try_init()
        .ok()?;
    Some(guard)
}

/// MCP clients `spacehog mcp install` knows how to configure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    ClaudeCode,
    ClaudeDesktop,
    Cursor,
}

/// Prints or writes the config snippet for a client. Returns what it did.
pub fn install(client: Option<Client>, write: bool) -> anyhow::Result<String> {
    let exe = std::env::current_exe()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    install::install_in(&spacehog_core::paths::home_dir(), &exe, client, write)
}
