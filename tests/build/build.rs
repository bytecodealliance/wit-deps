use anyhow::Context;
use tracing_subscriber::prelude::*;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .pretty()
                .without_time()
                .with_writer(std::io::stderr),
        )
        .with(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::DEBUG.into())
                .from_env_lossy(),
        )
        .init();

    depit::lock_sync!().context("failed to lock root WIT dependencies")?;

    println!("cargo:rerun-if-changed=wit/deps");
    println!("cargo:rerun-if-changed=wit/deps.lock");
    println!("cargo:rerun-if-changed=wit/deps.toml");

    Ok(())
}
