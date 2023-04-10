#![warn(clippy::pedantic)]

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use depit::Identifier;
use tokio::fs::File;
use tokio::io;
use tokio_util::compat::TokioAsyncWriteCompatExt;
use tracing_subscriber::prelude::*;

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Dependency output directory
    #[arg(short, long, default_value = "wit/deps")]
    deps: PathBuf,

    /// Dependency manifest path
    #[arg(short, long, default_value = "wit/deps.toml")]
    manifest: PathBuf,

    /// Dependency lock path
    #[arg(short, long, default_value = "wit/deps.lock")]
    lock: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Lock dependencies
    Lock {
        /// Optional list of packages to lock
        #[arg(short, long)]
        package: Vec<Identifier>,
    },
    /// Write a deterministic tar containing the `wit` subdirectory for a package to stdout
    Tar {
        /// Package to archive
        package: Identifier,

        /// Optional output path, if not specified, the archive will be written to stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .compact()
                .without_time()
                .with_file(false)
                .with_target(false)
                .with_writer(std::io::stderr),
        )
        .with(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();

    let Cli {
        deps: deps_path,
        manifest: manifest_path,
        lock: lock_path,
        command,
    } = Cli::parse();

    match command {
        None => depit::lock_path(None, manifest_path, lock_path, deps_path, None)
            .await
            .map(|_| ()),
        Some(Command::Lock { package }) => {
            depit::lock_path(None, manifest_path, lock_path, deps_path, &package)
                .await
                .map(|_| ())
        }
        Some(Command::Tar { package, output }) => {
            depit::lock_path(None, manifest_path, lock_path, &deps_path, [&package])
                .await
                .map(|_| ())?;
            let package = deps_path.join(package);
            if let Some(output) = output {
                let output = File::create(&output).await.with_context(|| {
                    format!("failed to create output path `{}`", output.display())
                })?;
                depit::tar(package, output.compat_write()).await?;
            } else {
                depit::tar(package, io::stdout().compat_write()).await?;
            }
            Ok(())
        }
    }
}
