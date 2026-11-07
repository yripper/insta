use clap::Parser;

// One thread is plenty for a local server that waits on Instagram, and keeps
// memory low.
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "insta=info".into()))
        .with_target(false)
        // pm2 writes stdout to log files: no color codes there.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();
    insta::cli::run(insta::cli::Cli::parse()).await
}
