//! `orkestrd` — the headless Orkestr core (§12).
//!
//! ```text
//! orkestrd [--data-dir <path>] [--port <port>] [--log-json] [--background]
//! ```
//!
//! Prints the RPC URL on startup, writes `<data_dir>/core.json` for discovery
//! and shuts down gracefully on Ctrl-C, SIGTERM/SIGHUP (Unix) or console
//! close / logoff / system shutdown (Windows) — see
//! `orkestr_platform::signals`. `--background` (used by the autostart entry)
//! suppresses the console banner. Autostart registered from this process
//! starts `orkestrd --background [--data-dir <path>]`.

#![forbid(unsafe_code)]

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use anyhow::{bail, Context};
use orkestr_core::{logging, AutostartConfig, CoreConfig};

struct Args {
    data_dir: Option<PathBuf>,
    port: u16,
    log_json: bool,
    background: bool,
}

fn usage() -> &'static str {
    "orkestrd — headless Orkestr core\n\nUSAGE:\n  orkestrd [--data-dir <path>] [--port <port>] [--log-json] [--background]\n\n\
     ENV:\n  ORKESTR_HOME   data directory override\n  RUST_LOG       log filter (default: info)\n"
}

fn parse_args() -> anyhow::Result<Option<Args>> {
    let mut args = Args { data_dir: None, port: 0, log_json: false, background: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--data-dir" => args.data_dir = Some(it.next().context("--data-dir needs a value")?.into()),
            "--port" => args.port = it.next().context("--port needs a value")?.parse().context("invalid --port")?,
            "--log-json" => args.log_json = true,
            "--background" => args.background = true,
            "-h" | "--help" => {
                print!("{}", usage());
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("orkestrd {}", orkestr_core::VERSION);
                return Ok(None);
            }
            other => bail!("unknown argument `{other}`\n\n{}", usage()),
        }
    }
    Ok(Some(args))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let Some(args) = parse_args()? else { return Ok(()) };
    logging::init(args.log_json);
    let mut autostart_args = vec!["--background".to_owned()];
    if let Some(dir) = &args.data_dir {
        autostart_args.push("--data-dir".into());
        autostart_args.push(dir.display().to_string());
    }
    let config = CoreConfig {
        data_dir: args.data_dir,
        bind_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, args.port)),
        autostart: AutostartConfig {
            id: "dev.orkestr.orkestrd".into(),
            args: autostart_args,
            ..AutostartConfig::default()
        },
        ..CoreConfig::default()
    };
    let core = orkestr_core::start(config).await.context("failed to start orkestr core")?;
    if !args.background {
        println!("orkestrd {} listening on {}", orkestr_core::VERSION, core.url);
        println!("discovery: {}", core.data_dir.join("core.json").display());
    }
    let stopped = core.shutdown_token();
    tokio::select! {
        signal = orkestr_platform::shutdown_signal() => tracing::info!(%signal, "shutdown requested"),
        () = stopped.cancelled() => {},
    }
    core.shutdown().await;
    Ok(())
}
