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
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,wit_deps=trace")),
        )
        .init();

    // NOTE: Root WIT definitions are a dependency of this crate, so those have to be locked first
    wit_deps::lock_sync!("../wit").context("failed to lock root WIT dependencies")?;
    wit_deps::lock_sync!().context("failed to lock own WIT dependencies")?;

    println!("cargo:rerun-if-changed=wit/deps");
    println!("cargo:rerun-if-changed=wit/deps.lock");
    println!("cargo:rerun-if-changed=wit/deps.toml");

    println!("cargo:rerun-if-changed=../wit");

    Ok(())
}
