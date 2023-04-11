//! WIT dependency management core library

#![forbid(clippy::unwrap_used)]
#![warn(clippy::pedantic)]
#![warn(missing_docs)]

mod cache;
mod digest;
mod lock;
mod manifest;

pub use cache::{Cache, Local as LocalCache, Write as WriteCache};
pub use digest::{Digest, Reader as DigestReader, Writer as DigestWriter};
pub use lock::{Entry as LockEntry, EntrySource as LockEntrySource, Lock};
pub use manifest::{Entry as ManifestEntry, Manifest};

pub use futures;
pub use tokio;

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::Path;

use anyhow::{bail, Context};
use futures::{try_join, AsyncRead, AsyncWrite, FutureExt, Stream, TryStreamExt};
use tokio::fs;
use tokio_stream::wrappers::ReadDirStream;
use tracing::{debug, instrument};

/// WIT dependency identifier
pub type Identifier = String;
// TODO: Introduce a rich type with name validation
//#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq)]
//pub struct Identifier(String);

fn is_wit(path: impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case("wit"))
        .unwrap_or_default()
}

/// Returns a stream of WIT file names within a directory at `path`
#[instrument(level = "trace", skip(path))]
async fn read_wits(
    path: impl AsRef<Path>,
) -> std::io::Result<impl Stream<Item = std::io::Result<OsString>>> {
    let st = fs::read_dir(path).await.map(ReadDirStream::new)?;
    Ok(st.try_filter_map(|e| async move {
        let name = e.file_name();
        if !is_wit(&name) {
            return Ok(None);
        }
        if e.file_type().await?.is_dir() {
            return Ok(None);
        }
        Ok(Some(name))
    }))
}

/// Copies all WIT files from directory at `src` to `dst`
#[instrument(level = "trace", skip(src, dst))]
async fn copy_wits(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();
    read_wits(src)
        .await
        .map_err(|e| std::io::Error::new(e.kind(), format!("failed to read `{}`", src.display())))?
        .try_for_each_concurrent(None, |name| async {
            let src = src.join(&name);
            let dst = dst.join(name);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent).await.map_err(|e| {
                    std::io::Error::new(
                        e.kind(),
                        format!(
                            "failed to create destination parent directory `{}`: {e}",
                            parent.display()
                        ),
                    )
                })?;
            }
            fs::copy(&src, &dst).await.map(|_| ()).map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!(
                        "failed to copy `{}` to `{}`: {e}",
                        src.display(),
                        dst.display()
                    ),
                )
            })
        })
        .await
}

/// Unpacks all WIT interfaces found within `wit` subtree of a tar archive read from `tar` to `dst`
///
/// # Errors
///
/// Returns and error if the operation fails
#[instrument(level = "trace", skip(tar, dst))]
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
                    if is_wit(name) =>
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
#[instrument(level = "trace", skip(path, dst))]
pub async fn tar<T>(path: impl AsRef<Path>, dst: T) -> std::io::Result<T>
where
    T: AsyncWrite + Sync + Send + Unpin,
{
    let path = path.as_ref();
    let mut tar = async_tar::Builder::new(dst);
    tar.mode(async_tar::HeaderMode::Deterministic);
    for name in read_wits(path).await?.try_collect::<BTreeSet<_>>().await? {
        tar.append_path_with_name(path.join(&name), Path::new("wit").join(name))
            .await?;
    }
    tar.into_inner().await
}

fn cache() -> Option<impl Cache> {
    LocalCache::cache_dir().map(|cache| {
        debug!("using cache at `{cache}`");
        cache
    })
}

/// Given a TOML-encoded manifest and optional TOML-encoded lock, ensures that the path pointed to by
/// `deps` is in sync with the manifest and lock. This is a potentially destructive operation!
/// Returns a TOML-encoded lock if the lock passed to this function was either `None` or out-of-sync.
///
/// # Errors
///
/// Returns an error if anything in the pipeline fails
#[instrument(level = "trace", skip(at, manifest, lock, deps, packages))]
pub async fn lock(
    at: Option<impl AsRef<Path>>,
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

    let deps = deps.as_ref();
    let lock = manifest
        .lock(at, deps, old_lock.as_ref(), cache().as_ref(), packages)
        .await
        .with_context(|| format!("failed to lock deps to `{}`", deps.display()))?;
    match old_lock {
        Some(old_lock) if lock == old_lock => Ok(None),
        _ => toml::to_string(&lock)
            .map(Some)
            .context("failed to encode lock"),
    }
}

/// Given a TOML-encoded manifest, ensures that the path pointed to by
/// `deps` is in sync with the manifest. This is a potentially destructive operation!
/// Returns a TOML-encoded lock on success.
///
/// # Errors
///
/// Returns an error if anything in the pipeline fails
#[instrument(level = "trace", skip(at, manifest, deps, packages))]
pub async fn update(
    at: Option<impl AsRef<Path>>,
    manifest: impl AsRef<str>,
    deps: impl AsRef<Path>,
    packages: impl IntoIterator<Item = &Identifier>,
) -> anyhow::Result<String> {
    let manifest: Manifest =
        toml::from_str(manifest.as_ref()).context("failed to decode manifest")?;

    let deps = deps.as_ref();
    let lock = manifest
        .lock(at, deps, None, cache().map(WriteCache).as_ref(), packages)
        .await
        .with_context(|| format!("failed to lock deps to `{}`", deps.display()))?;
    toml::to_string(&lock).context("failed to encode lock")
}

async fn read_manifest_string(path: impl AsRef<Path>) -> std::io::Result<String> {
    let path = path.as_ref();
    fs::read_to_string(&path).await.map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("failed to read manifest at `{}`: {e}", path.display()),
        )
    })
}

async fn write_lock(path: impl AsRef<Path>, buf: impl AsRef<[u8]>) -> std::io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await.map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!(
                    "failed to create lock parent directory `{}`: {e}",
                    parent.display()
                ),
            )
        })?;
    }
    fs::write(&path, &buf).await.map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("failed to write lock to `{}`: {e}", path.display()),
        )
    })
}

/// Like [lock](self::lock()), but reads the manifest at `manifest_path` and reads/writes the lock at `lock_path`.
///
/// Returns `true` if the lock was updated and `false` otherwise.
///
/// # Errors
///
/// Returns an error if anything in the pipeline fails
#[instrument(level = "trace", skip(manifest_path, lock_path, deps, packages))]
pub async fn lock_path(
    manifest_path: impl AsRef<Path>,
    lock_path: impl AsRef<Path>,
    deps: impl AsRef<Path>,
    packages: impl IntoIterator<Item = &Identifier>,
) -> anyhow::Result<bool> {
    let manifest_path = manifest_path.as_ref();
    let lock_path = lock_path.as_ref();
    let (manifest, lock) = try_join!(
        read_manifest_string(manifest_path),
        fs::read_to_string(&lock_path).map(|res| match res {
            Ok(lock) => Ok(Some(lock)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(std::io::Error::new(
                e.kind(),
                format!("failed to read lock at `{}`: {e}", lock_path.display())
            )),
        }),
    )?;
    if let Some(lock) = self::lock(manifest_path.parent(), manifest, lock, deps, packages)
        .await
        .context("failed to lock dependencies")?
    {
        write_lock(lock_path, lock).await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Like [update](self::update()), but reads the manifest at `manifest_path` and writes the lock at `lock_path`.
///
/// # Errors
///
/// Returns an error if anything in the pipeline fails
#[instrument(level = "trace", skip(manifest_path, lock_path, deps, packages))]
pub async fn update_path(
    manifest_path: impl AsRef<Path>,
    lock_path: impl AsRef<Path>,
    deps: impl AsRef<Path>,
    packages: impl IntoIterator<Item = &Identifier>,
) -> anyhow::Result<()> {
    let manifest_path = manifest_path.as_ref();
    let manifest = read_manifest_string(manifest_path).await?;
    let lock = self::update(manifest_path.parent(), manifest, deps, packages)
        .await
        .context("failed to lock dependencies")?;
    write_lock(lock_path, lock).await?;
    Ok(())
}

/// Asynchronously ensure dependency manifest, lock and dependencies are in sync.
/// This must run within a [tokio] context.
#[macro_export]
macro_rules! lock {
    () => {
        $crate::lock!("wit")
    };
    ($dir:literal $(,)?) => {
        async {
            use $crate::tokio::fs;

            use std::io::{Error, ErrorKind};

            let lock = match fs::read_to_string(concat!($dir, "/deps.lock")).await {
                Ok(lock) => Some(lock),
                Err(e) if e.kind() == ErrorKind::NotFound => None,
                Err(e) => {
                    return Err(Error::new(
                        e.kind(),
                        format!(
                            "failed to read lock at `{}`: {e}",
                            concat!($dir, "/deps.lock")
                        ),
                    ))
                }
            };
            match $crate::lock(
                Some($dir),
                include_str!(concat!($dir, "/deps.toml")),
                lock,
                concat!($dir, "/deps"),
                None,
            )
            .await
            {
                Ok(Some(lock)) => fs::write(concat!($dir, "/deps.lock"), lock)
                    .await
                    .map_err(|e| {
                        Error::new(
                            e.kind(),
                            format!(
                                "failed to write lock at `{}`: {e}",
                                concat!($dir, "/deps.lock")
                            ),
                        )
                    }),
                Ok(None) => Ok(()),
                Err(e) => Err(Error::new(ErrorKind::Other, e)),
            }
        }
    };
}

#[cfg(feature = "sync")]
/// Synchronously ensure dependency manifest, lock and dependencies are in sync.
#[macro_export]
macro_rules! lock_sync {
    ($($args:tt)*) => {
        $crate::tokio::runtime::Builder::new_multi_thread()
            .thread_name("wit-deps/lock_sync")
            .enable_io()
            .enable_time()
            .build()?
            .block_on($crate::lock!($($args)*))
    };
}
