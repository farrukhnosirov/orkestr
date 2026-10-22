//! `orkestr-gateway` — self-hosted Orkestr gateway (Phase 4).
#![forbid(unsafe_code)]

use orkestr_gateway::{run, GatewayConfig};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let cfg = match GatewayConfig::from_args(std::env::args().skip(1)) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("orkestr-gateway: {e}");
            return std::process::ExitCode::from(2);
        }
    };
    match run(cfg).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("orkestr-gateway: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
