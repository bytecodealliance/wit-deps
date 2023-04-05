//! Depit core

#![forbid(clippy::unwrap_used)]
#![warn(clippy::pedantic)]
#![warn(missing_docs)]

mod cache;
mod digest;
mod lock;
mod manifest;

pub use cache::{Cache, Local as LocalCache};
pub use digest::{Digest, Reader as DigestReader, Writer as DigestWriter};
pub use lock::{Entry as LockEntry, Lock};
pub use manifest::{Entry as ManifestEntry, Manifest};

use std::ffi::OsStr;
use std::path::Path;

use anyhow::{bail, Context};
use directories::ProjectDirs;
use futures::{AsyncRead, AsyncWrite, TryStreamExt};
use tokio::fs;
use tracing::debug;

/// WIT dependency identifier
pub type Identifier = String;
// TODO: Introduce a rich type with name validation
//#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq)]
//pub struct Identifier(String);

/// Unpacks all WIT interfaces found within `wit` subtree of a tar archive read from `tar` to `dst`
///
/// # Errors
///
/// Returns and error if the operation fails
pub async fn untar(tar: impl AsyncRead + Unpin, dst: impl AsRef<Path>) -> anyhow::Result<()> {
    let dst = dst.as_ref();

    match fs::remove_dir_all(dst).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => bail!("failed to remove `{}`: {e}", dst.display()),
    };
    fs::create_dir_all(dst)
        .await
        .with_context(|| format!("failed to create `{}`", dst.display()))?;

    async_tar::Archive::new(tar)
        .entries()
        .context("failed to unpack archive metadata")?
        .try_for_each(|mut e| async move {
            let path = e.path()?;
            let mut path = path.into_iter().map(OsStr::to_str);
            match (path.next(), path.next(), path.next(), path.next()) {
                (Some(Some("wit")), Some(Some(name)), None, None)
                | (Some(_), Some(Some("wit")), Some(Some(name)), None)
                    if Path::new(name)
                        .extension()
                        .map(|ext| ext.eq_ignore_ascii_case("wit"))
                        .unwrap_or_default() =>
                {
                    e.unpack(dst.join(name)).await?;
                    Ok(())
                }
                _ => Ok(()),
            }
        })
        .await
        .context("failed to unpack archive")?;
    Ok(())
}

/// Packages path into a `wit` subtree in deterministic `tar` archive and writes it to `dst`.
///
/// # Errors
///
/// Returns and error if the operation fails
pub async fn tar<T>(path: impl AsRef<Path>, dst: T) -> std::io::Result<T>
where
    T: AsyncWrite + Sync + Send + Unpin,
{
    let path = path.as_ref();
    let mut tar = async_tar::Builder::new(dst);
    tar.mode(async_tar::HeaderMode::Deterministic);
    tar.append_dir_all("wit", path).await?;
    tar.into_inner().await
}

/// Given a manifest TOML string and optional lock TOML string, ensures that the path pointed to by
/// `deps` contains expected contents. This is a potentially destructive operation!
/// Returns a lock if the lock passed to this function was either `None` or out-of-sync.
///
/// # Errors
///
/// Returns an error if anything in the pipeline fails
pub async fn lock(
    manifest: impl AsRef<str>,
    lock: Option<impl AsRef<str>>,
    deps: impl AsRef<Path>,
    packages: impl IntoIterator<Item = &Identifier>,
) -> anyhow::Result<Option<String>> {
    let manifest: Manifest =
        toml::from_str(manifest.as_ref()).context("failed to decode manifest")?;

    let old_lock = lock
        .as_ref()
        .map(AsRef::as_ref)
        .map(toml::from_str)
        .transpose()
        .context("failed to decode lock")?;

    let dirs = ProjectDirs::from("", "", env!("CARGO_PKG_NAME"));
    let cache = dirs.as_ref().map(ProjectDirs::cache_dir).map(|cache| {
        debug!("using cache at `{}`", cache.display());
        LocalCache::from(cache)
    });

    let deps = deps.as_ref();
    let lock = manifest
        .lock(deps, old_lock.as_ref(), cache.as_ref(), packages)
        .await
        .with_context(|| format!("failed to lock deps to `{}`", deps.display()))?;

    match old_lock {
        Some(old_lock) if lock == old_lock => Ok(None),
        _ => {
            let lock = toml::to_string(&lock).context("failed to encode lock")?;
            Ok(Some(lock))
        }
    }
}

/// Ensure dependency manifest, lock and packages are in sync
#[macro_export]
macro_rules! lock {
    () => {
        $crate::lock(
            include_str!("wit/deps.toml"),
            Some(include_str!("wit/deps.lock")),
            "wit/deps",
            None,
        )
    };
}
