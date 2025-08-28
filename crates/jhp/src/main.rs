use clap::Parser;
use jhp_engine::config::EngineConfig;
use jhp_engine::engine::Engine;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "jhp",
    version,
    about = "JHP engine",
    disable_help_subcommand = true
)]
struct Cli {
    /// Start the built-in HTTP server at HOST:PORT
    #[arg(short = 'S', value_name = "HOST:PORT")]
    serve: Option<String>,

    /// Set the document root to serve from
    #[arg(short = 't', long = "docroot", value_name = "DIR")]
    docroot: Option<PathBuf>,
}

fn parse_host_port(s: &str) -> Result<(String, u16), String> {
    if let Some(rest) = s.strip_prefix('[') {
        // bracketed IPv6: [host]:port
        if let Some(end) = rest.find(']') {
            let host = &rest[..end];
            let remain = &rest[end + 1..];
            let port = remain
                .strip_prefix(':')
                .ok_or("missing port after IPv6 host")?;
            let port: u16 = port.parse().map_err(|_| "invalid port".to_string())?;
            return Ok((host.to_string(), port));
        }
        return Err("invalid bracketed IPv6 address".to_string());
    }
    let mut parts = s.rsplitn(2, ':');
    let port_str = parts.next().ok_or("missing port")?;
    let host = parts.next().ok_or("missing host")?;
    let port: u16 = port_str.parse().map_err(|_| "invalid port".to_string())?;
    Ok((host.to_string(), port))
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let mut config = EngineConfig::default();
    if let Some(addr) = cli.serve.as_deref() {
        match parse_host_port(addr) {
            Ok((host, port)) => {
                config.host = host;
                config.port = port;
            }
            Err(e) => {
                eprintln!("-S expects HOST:PORT (e.g. 127.0.0.1:3000), error: {}", e);
                std::process::exit(2);
            }
        }
    }

    if let Some(docroot) = cli.docroot {
        config = config.set_document_root(docroot);
    }

    let mut engine = Engine::new_with_config(4, config);
    engine.run().await.unwrap();
}
