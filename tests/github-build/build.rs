use anyhow::Context;

fn main() -> anyhow::Result<()> {
    depit::lock_sync!().context("failed to lock WIT dependencies")?;
    depit::lock_sync!("wit").context("failed to lock WIT dependencies")?;

    println!("cargo:rerun-if-changed=wit/deps");
    println!("cargo:rerun-if-changed=wit/deps.lock");
    println!("cargo:rerun-if-changed=wit/deps.toml");

    Ok(())
}
