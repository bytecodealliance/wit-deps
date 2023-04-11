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

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use anyhow::Context;
use futures::{try_join, AsyncRead, AsyncWrite, FutureExt, Stream, TryStreamExt};
use tokio::fs;
use tokio_stream::wrappers::ReadDirStream;
use tracing::{debug, instrument, trace};

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

#[instrument(level = "trace", skip(path))]
async fn remove_dir_all(path: impl AsRef<Path>) -> std::io::Result<()> {
    let path = path.as_ref();
    match fs::remove_dir_all(path).await {
        Ok(()) => {
            trace!("removed `{}`", path.display());
            Ok(())
        }
        Err(e) => Err(std::io::Error::new(
            e.kind(),
            format!("failed to remove `{}`: {e}", path.display()),
        )),
    }
}

#[instrument(level = "trace", skip(path))]
async fn recreate_dir(path: impl AsRef<Path>) -> std::io::Result<()> {
    let path = path.as_ref();
    match remove_dir_all(path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    };
    fs::create_dir_all(path)
        .await
        .map(|()| trace!("recreated `{}`", path.display()))
        .map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("failed to create `{}`: {e}", path.display()),
            )
        })
}

/// Returns a stream of WIT file names within a directory at `path`
#[instrument(level = "trace", skip(path))]
async fn read_wits(
    path: impl AsRef<Path>,
) -> std::io::Result<impl Stream<Item = std::io::Result<OsString>>> {
    let path = path.as_ref();
    let st = fs::read_dir(path)
        .await
        .map(ReadDirStream::new)
        .map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("failed to read directory at `{}`: {e}", path.display()),
            )
        })?;
    Ok(st.try_filter_map(|e| async move {
        let name = e.file_name();
        if !is_wit(&name) {
            trace!("{} is not a WIT definition, skip", name.to_string_lossy());
            return Ok(None);
        }
        if e.file_type().await?.is_dir() {
            trace!("{} is a directory, skip", name.to_string_lossy());
            return Ok(None);
        }
        Ok(Some(name))
    }))
}

/// Copies all WIT definitions from directory at `src` to `dst` creating `dst` directory, if it does not exist.
#[instrument(level = "trace", skip(src, dst))]
async fn install_wits(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();
    recreate_dir(dst).await?;
    read_wits(src)
        .await?
        .try_for_each_concurrent(None, |name| async {
            let src = src.join(&name);
            let dst = dst.join(name);
            fs::copy(&src, &dst)
                .await
                .map(|_| trace!("copied `{}` to `{}`", src.display(), dst.display()))
                .map_err(|e| {
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

/// Copies all WIT files from directory at `src` to `dst` and returns a vector identifiers of all copied
/// transitive dependencies.
#[instrument(level = "trace", skip(src, dst, skip_deps))]
async fn copy_wits(
    src: impl AsRef<Path>,
    dst: impl AsRef<Path>,
    skip_deps: &HashSet<Identifier>,
) -> std::io::Result<HashMap<Identifier, PathBuf>> {
    let src = src.as_ref();
    let deps = src.join("deps");
    let dst = dst.as_ref();
    try_join!(install_wits(src, dst), async {
        match (dst.parent(), fs::read_dir(&deps).await) {
            (Some(base), Ok(dir)) => {
                ReadDirStream::new(dir)
                    .try_filter_map(|e| async move {
                        let name = e.file_name();
                        let Some(id) = name.to_str().map(Identifier::from) else {
                            return Ok(None)
                        };
                        if skip_deps.contains(&id) {
                            return Ok(None);
                        }
                        let ft = e.file_type().await?;
                        if !(ft.is_dir()
                            || ft.is_symlink() && fs::metadata(e.path()).await?.is_dir())
                        {
                            return Ok(None);
                        }
                        Ok(Some(id))
                    })
                    .and_then(|id| async {
                        let dst = base.join(&id);
                        install_wits(deps.join(&id), &dst).await?;
                        Ok((id, dst))
                    })
                    .try_collect()
                    .await
            }
            (None, _) => Ok(HashMap::default()),
            (_, Err(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::default()),
            (_, Err(e)) => Err(std::io::Error::new(
                e.kind(),
                format!("failed to read directory at `{}`: {e}", deps.display()),
            )),
        }
    })
    .map(|((), ids)| ids)
}

/// Unpacks all WIT interfaces found within `wit` subtree of a tar archive read from `tar` to
/// `dst` and returns a vector of all unpacked transitive dependency identifiers.
///
/// # Errors
///
/// Returns and error if the operation fails
#[instrument(level = "trace", skip(tar, dst, skip_deps))]
pub async fn untar(
    tar: impl AsyncRead + Unpin,
    dst: impl AsRef<Path>,
    skip_deps: &HashSet<Identifier>,
) -> std::io::Result<HashMap<Identifier, PathBuf>> {
    use std::io::{Error, Result};

    async fn unpack(e: &mut async_tar::Entry<impl Unpin + AsyncRead>, dst: &Path) -> Result<()> {
        e.unpack(dst).await.map_err(|e| {
            Error::new(
                e.kind(),
                format!("failed to unpack `{}`: {e}", dst.display()),
            )
        })?;
        trace!("unpacked `{}`", dst.display());
        Ok(())
    }

    let dst = dst.as_ref();
    recreate_dir(dst).await?;
    async_tar::Archive::new(tar)
        .entries()
        .map_err(|e| Error::new(e.kind(), format!("failed to unpack archive metadata: {e}")))?
        .try_filter_map(|mut e| async move {
            let path = e
                .path()
                .map_err(|e| Error::new(e.kind(), format!("failed to query entry path: {e}")))?;
            let mut path = path.into_iter().map(OsStr::to_str);
            match (
                path.next(),
                path.next(),
                path.next(),
                path.next(),
                path.next(),
            ) {
                (Some(Some("wit")), Some(Some(name)), None, None, None)
                | (Some(_), Some(Some("wit")), Some(Some(name)), None, None)
                    if is_wit(name) =>
                {
                    let dst = dst.join(name);
                    unpack(&mut e, &dst).await?;
                    Ok(None)
                }
                (Some(Some("wit")), Some(Some("deps")), Some(Some(id)), Some(Some(name)), None)
                | (
                    Some(_),
                    Some(Some("wit")),
                    Some(Some("deps")),
                    Some(Some(id)),
                    Some(Some(name)),
                ) if !skip_deps.contains(id) && is_wit(name) => {
                    let id = Identifier::from(id);
                    if let Some(base) = dst.parent() {
                        let dst = base.join(&id);
                        recreate_dir(&dst).await?;
                        let wit = dst.join(name);
                        unpack(&mut e, &wit).await?;
                        Ok(Some((id, dst)))
                    } else {
                        Ok(None)
                    }
                }
                _ => Ok(None),
            }
        })
        .try_collect()
        .await
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
#[instrument(level = "trace", skip(at, manifest, lock, deps))]
pub async fn lock(
    at: Option<impl AsRef<Path>>,
    manifest: impl AsRef<str>,
    lock: Option<impl AsRef<str>>,
    deps: impl AsRef<Path>,
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
        .lock(at, deps, old_lock.as_ref(), cache().as_ref())
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
#[instrument(level = "trace", skip(at, manifest, deps))]
pub async fn update(
    at: Option<impl AsRef<Path>>,
    manifest: impl AsRef<str>,
    deps: impl AsRef<Path>,
) -> anyhow::Result<String> {
    let manifest: Manifest =
        toml::from_str(manifest.as_ref()).context("failed to decode manifest")?;

    let deps = deps.as_ref();
    let lock = manifest
        .lock(at, deps, None, cache().map(WriteCache).as_ref())
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
#[instrument(level = "trace", skip(manifest_path, lock_path, deps))]
pub async fn lock_path(
    manifest_path: impl AsRef<Path>,
    lock_path: impl AsRef<Path>,
    deps: impl AsRef<Path>,
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
    if let Some(lock) = self::lock(manifest_path.parent(), manifest, lock, deps)
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
#[instrument(level = "trace", skip(manifest_path, lock_path, deps))]
pub async fn update_path(
    manifest_path: impl AsRef<Path>,
    lock_path: impl AsRef<Path>,
    deps: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let manifest_path = manifest_path.as_ref();
    let manifest = read_manifest_string(manifest_path).await?;
    let lock = self::update(manifest_path.parent(), manifest, deps)
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
