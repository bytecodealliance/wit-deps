use crate::{
    copy_wits, remove_dir_all, untar, Cache, Digest, DigestReader, Identifier, Lock, LockEntry,
    LockEntrySource,
};

use core::convert::identity;
use core::convert::Infallible;
use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::ensure;
use anyhow::{bail, Context as _};
use async_compression::futures::bufread::GzipDecoder;
use futures::io::BufReader;
use futures::lock::Mutex;
use futures::{stream, AsyncWriteExt, StreamExt, TryStreamExt};
use hex::FromHex;
use serde::{de, Deserialize};
use tracing::{debug, error, info, instrument, trace, warn};
use url::Url;

/// WIT dependency [Manifest] entry
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Entry {
    /// Dependency specification expressed as a resource (typically, a gzipped tarball) URL
    Url {
        /// Resource URL
        url: Url,
        /// Optional sha256 digest of this resource
        sha256: Option<[u8; 32]>,
        /// Optional sha512 digest of this resource
        sha512: Option<[u8; 64]>,
        /// Subdirectory within resource containing WIT, `wit` by default
        subdir: Box<str>,
    },
    /// Dependency specification expressed as a local path to a directory containing WIT
    /// definitions
    Path(PathBuf),
    // TODO: Support semver queries
}

impl From<Url> for Entry {
    fn from(url: Url) -> Self {
        Self::Url {
            url,
            sha256: None,
            sha512: None,
            subdir: "wit".into(),
        }
    }
}

impl From<PathBuf> for Entry {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

impl FromStr for Entry {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.parse().ok().filter(|url: &Url| !url.cannot_be_a_base()) {
            Some(url) => Ok(Self::from(url)),
            None => Ok(Self::from(PathBuf::from(s))),
        }
    }
}

impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        const FIELDS: [&str; 4] = ["path", "sha256", "sha512", "url"];

        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = Entry;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a WIT dependency manifest entry")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                value.parse().map_err(de::Error::custom)
            }

            fn visit_map<V>(self, mut map: V) -> Result<Self::Value, V::Error>
            where
                V: de::MapAccess<'de>,
            {
                let mut path = None;
                let mut sha256 = None;
                let mut sha512 = None;
                let mut subdir: Option<String> = None;
                let mut url = None;
                while let Some((k, v)) = map.next_entry::<String, String>()? {
                    match k.as_ref() {
                        "path" => {
                            if path.is_some() {
                                return Err(de::Error::duplicate_field("path"));
                            }
                            path = v.parse().map(Some).map_err(|e| {
                                de::Error::custom(format!("invalid `path` field value: {e}"))
                            })?;
                        }
                        "sha256" => {
                            if sha256.is_some() {
                                return Err(de::Error::duplicate_field("sha256"));
                            }
                            sha256 = FromHex::from_hex(v).map(Some).map_err(|e| {
                                de::Error::custom(format!("invalid `sha256` field value: {e}"))
                            })?;
                        }
                        "sha512" => {
                            if sha512.is_some() {
                                return Err(de::Error::duplicate_field("sha512"));
                            }
                            sha512 = FromHex::from_hex(v).map(Some).map_err(|e| {
                                de::Error::custom(format!("invalid `sha512` field value: {e}"))
                            })?;
                        }
                        "subdir" => {
                            if subdir.is_some() {
                                return Err(de::Error::duplicate_field("subdir"));
                            }
                            subdir = v.parse().map(Some).map_err(|e| {
                                de::Error::custom(format!("invalid `subdir` field value: {e}"))
                            })?;
                        }
                        "url" => {
                            if url.is_some() {
                                return Err(de::Error::duplicate_field("url"));
                            }
                            url = v.parse().map(Some).map_err(|e| {
                                de::Error::custom(format!("invalid `url` field value: {e}"))
                            })?;
                        }
                        k => return Err(de::Error::unknown_field(k, &FIELDS)),
                    }
                }
                match (path, sha256, sha512, subdir, url) {
                    (Some(path), None, None, None, None) => Ok(Entry::Path(path)),
                    (None, sha256, sha512, None, Some(url)) => Ok(Entry::Url {
                        url,
                        sha256,
                        sha512,
                        subdir: "wit".into(),
                    }),
                    (None, sha256, sha512, Some(subdir), Some(url)) => Ok(Entry::Url {
                        url,
                        sha256,
                        sha512,
                        subdir: subdir.into_boxed_str(),
                    }),
                    (Some(_), None | Some(_), None | Some(_), None | Some(_), None) => {
                        Err(de::Error::custom(
                            "`subdir`, `sha256` and `sha512` are not supported in combination with `path`",
                        ))
                    }
                    _ => Err(de::Error::custom("eiter `url` or `path` must be specified")),
                }
            }
        }
        deserializer.deserialize_struct("Entry", &FIELDS, Visitor)
    }
}

fn source_matches(
    digest: impl Into<Digest>,
    sha256: Option<[u8; 32]>,
    sha512: Option<[u8; 64]>,
) -> bool {
    let digest = digest.into();
    sha256.map_or(true, |sha256| sha256 == digest.sha256)
        && sha512.map_or(true, |sha512| sha512 == digest.sha512)
}

#[instrument(level = "trace", skip(deps))]
async fn lock_deps(
    deps: impl IntoIterator<Item = (Identifier, PathBuf)>,
) -> anyhow::Result<HashMap<Identifier, LockEntry>> {
    stream::iter(deps.into_iter().map(|(id, path)| async {
        let entry = LockEntry::from_transitive_path(path).await?;
        Ok((id, entry))
    }))
    .then(identity)
    .try_collect()
    .await
}

impl Entry {
    #[instrument(level = "trace", skip(at, out, lock, cache, skip_deps))]
    async fn lock(
        self,
        at: Option<impl AsRef<Path>>,
        out: impl AsRef<Path>,
        lock: Option<&LockEntry>,
        cache: Option<&impl Cache>,
        skip_deps: &HashSet<Identifier>,
    ) -> anyhow::Result<(LockEntry, HashMap<Identifier, LockEntry>)> {
        let out = out.as_ref();
        let proxy_url = env::var("PROXY_SERVER").ok();
        let proxy_username = env::var("PROXY_USERNAME").ok();
        let proxy_password = env::var("PROXY_PASSWORD").ok();
        let http_client = if let (Some(proxy_url), Some(proxy_username), Some(proxy_password)) =
            (proxy_url, proxy_username, proxy_password)
        {
            let proxy_with_auth = format!(
                "http://{}:{}@{}",
                urlencoding::encode(&proxy_username),
                urlencoding::encode(&proxy_password),
                proxy_url
            );
            let proxy = reqwest::Proxy::all(proxy_with_auth)
                .context("failed to construct HTTP proxy configuration")?;
            reqwest::Client::builder()
                .proxy(proxy)
                .build()
                .context("failed to create HTTP client")?
        } else {
            reqwest::Client::new()
        };

        let entry = if let Some(LockEntry {
            source,
            digest: ldigest,
            deps: ldeps,
        }) = lock
        {
            let deps = if ldeps.is_empty() {
                Ok(HashMap::default())
            } else {
                let base = out
                    .parent()
                    .with_context(|| format!("`{}` does not have a parent", out.display()))?;
                lock_deps(ldeps.iter().cloned().map(|id| {
                    // Sanitize dependency name for filesystem compatibility
                    let sanitized_id = id.replace(":", "_");
                    let path = base.join(&sanitized_id);
                    (id, path)
                }))
                .await
            };
            match (LockEntry::digest(out).await, source, deps) {
                (Ok(digest), Some(source), Ok(deps)) if digest == *ldigest => {
                    // NOTE: Manually deleting transitive dependencies of this
                    // dependency from `dst` is considered user error
                    // TODO: Check that transitive dependencies are in sync
                    match (self, source) {
                        (
                            Self::Url { url, subdir, .. },
                            LockEntrySource::Url {
                                url: lurl,
                                subdir: lsubdir,
                            },
                        ) if url == *lurl && subdir == *lsubdir => {
                            debug!("`{}` is already up-to-date, skip fetch", out.display());
                            return Ok((
                                LockEntry::new(
                                    Some(LockEntrySource::Url { url, subdir }),
                                    digest,
                                    deps.keys().cloned().collect(),
                                ),
                                deps,
                            ));
                        }
                        (Self::Path(path), LockEntrySource::Path { path: lpath })
                            if path == *lpath =>
                        {
                            debug!("`{}` is already up-to-date, skip copy", out.display());
                            return Ok((
                                LockEntry::new(
                                    Some(LockEntrySource::Path { path }),
                                    digest,
                                    deps.keys().cloned().collect(),
                                ),
                                deps,
                            ));
                        }
                        (entry, _) => {
                            debug!("source mismatch");
                            entry
                        }
                    }
                }
                (Ok(digest), _, _) => {
                    debug!(
                        "`{}` is out-of-date (sha256: {})",
                        out.display(),
                        hex::encode(digest.sha256)
                    );
                    self
                }
                (Err(e), _, _) if e.kind() == std::io::ErrorKind::NotFound => {
                    debug!("locked dependency for `{}` missing", out.display());
                    self
                }
                (Err(e), _, _) => {
                    error!(
                        "failed to compute dependency digest for `{}`: {e}",
                        out.display()
                    );
                    self
                }
            }
        } else {
            self
        };
        match entry {
            Self::Path(path) => {
                let src = at.map(|at| at.as_ref().join(&path));
                let src = src.as_ref().unwrap_or(&path);
                let deps = copy_wits(src, out, skip_deps).await?;
                trace!(?deps, "copied WIT definitions to `{}`", out.display());
                let deps = lock_deps(deps).await?;
                trace!(
                    ?deps,
                    "locked transitive dependencies of `{}`",
                    out.display()
                );
                let digest = LockEntry::digest(out).await?;
                Ok((
                    LockEntry::new(
                        Some(LockEntrySource::Path { path }),
                        digest,
                        deps.keys().cloned().collect(),
                    ),
                    deps,
                ))
            }
            Self::Url {
                url,
                sha256,
                sha512,
                subdir,
            } => {
                let cache = if let Some(cache) = cache {
                    match cache.get(&url).await {
                        Err(e) => error!("failed to get `{url}` from cache: {e}"),
                        Ok(None) => debug!("`{url}` not present in cache"),
                        Ok(Some(tar_gz)) => {
                            let mut hashed = DigestReader::from(tar_gz);
                            match untar(
                                GzipDecoder::new(BufReader::new(&mut hashed)),
                                out,
                                skip_deps,
                                &subdir,
                            )
                            .await
                            {
                                Ok(deps) if source_matches(hashed, sha256, sha512) => {
                                    debug!("unpacked `{url}` from cache");
                                    let deps = lock_deps(deps).await?;
                                    let entry = LockEntry::from_url(
                                        url,
                                        out,
                                        deps.keys().cloned().collect(),
                                        subdir,
                                    )
                                    .await?;
                                    return Ok((entry, deps));
                                }
                                Ok(deps) => {
                                    warn!("cache hash mismatch for `{url}`");
                                    remove_dir_all(out).await?;
                                    for (_, dep) in deps {
                                        remove_dir_all(&dep).await?;
                                    }
                                }
                                Err(e) => {
                                    error!("failed to unpack `{url}` contents from cache: {e}");
                                }
                            }
                        }
                    }
                    if let Ok(cache) = cache.insert(&url).await {
                        Some(cache)
                    } else {
                        None
                    }
                } else {
                    None
                };
                let cache = Arc::new(Mutex::new(cache));
                let (digest, deps) = match url.scheme() {
                    "http" | "https" => {
                        info!("fetch `{url}` into `{}`", out.display());

                        let res = http_client
                            .get(url.clone())
                            .send()
                            .await
                            .context("failed to GET")
                            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
                            .error_for_status()
                            .context("GET request failed")
                            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                        let tar_gz = res
                            .bytes_stream()
                            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
                            .then(|chunk| async {
                                let chunk = chunk?;
                                let mut cache = cache.lock().await;
                                let cache_res = if let Some(w) = cache.as_mut().map(|w| async {
                                    if let Err(e) = w.write(&chunk).await {
                                        error!("failed to write chunk to cache: {e}");
                                        if let Err(e) = w.close().await {
                                            error!("failed to close cache writer: {e}");
                                        }
                                        return Err(e);
                                    }
                                    Ok(())
                                }) {
                                    Some(w.await)
                                } else {
                                    None
                                }
                                .transpose();
                                if cache_res.is_err() {
                                    // Drop the cache writer if a failure occurs
                                    cache.take();
                                }
                                Ok(chunk)
                            })
                            .into_async_read();
                        let mut hashed = DigestReader::from(Box::pin(tar_gz));
                        let deps = untar(
                            GzipDecoder::new(BufReader::new(&mut hashed)),
                            out,
                            skip_deps,
                            &subdir,
                        )
                        .await
                        .with_context(|| format!("failed to unpack contents of `{url}`"))?;
                        (Digest::from(hashed), deps)
                    }
                    "file" => bail!(
                        r#"`file` scheme is not supported for `url` field, use `path` instead. Try:

```
mydep = "/path/to/my/dep"
```

or

```
[mydep]
path = "/path/to/my/dep"
```
)"#
                    ),
                    scheme => bail!("unsupported URL scheme `{scheme}`"),
                };
                if let Some(sha256) = sha256 {
                    if digest.sha256 != sha256 {
                        remove_dir_all(out).await?;
                        bail!(
                            r#"sha256 hash mismatch for `{url}`
got: {}
expected: {}"#,
                            hex::encode(digest.sha256),
                            hex::encode(sha256),
                        );
                    }
                }
                if let Some(sha512) = sha512 {
                    if digest.sha512 != sha512 {
                        remove_dir_all(out).await?;
                        bail!(
                            r#"sha512 hash mismatch for `{url}`
got: {}
expected: {}"#,
                            hex::encode(digest.sha512),
                            hex::encode(sha512),
                        );
                    }
                }
                trace!(?deps, "fetched contents of `{url}` to `{}`", out.display());
                let deps = lock_deps(deps).await?;
                trace!(?deps, "locked transitive dependencies of `{url}`");
                let entry =
                    LockEntry::from_url(url, out, deps.keys().cloned().collect(), subdir).await?;
                Ok((entry, deps))
            }
        }
    }
}

/// WIT dependency manifest mapping [Identifiers](Identifier) to [Entries](Entry)
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Manifest(HashMap<Identifier, Entry>);

impl Manifest {
    /// Lock the manifest populating `deps`
    #[instrument(level = "trace", skip(at, deps, lock, cache))]
    pub async fn lock(
        self,
        at: Option<impl AsRef<Path>>,
        deps: impl AsRef<Path>,
        lock: Option<&Lock>,
        cache: Option<&impl Cache>,
    ) -> anyhow::Result<Lock> {
        let at = at.as_ref();
        let deps = deps.as_ref();
        // Dependency ids, which are pinned in the manifest
        let pinned = self.0.keys().cloned().collect();
        stream::iter(self.0.into_iter().map(|(id, entry)| async {
            let out = deps.join(&id);
            let lock = lock.and_then(|lock| lock.get(&id));
            let (entry, deps) = entry
                .lock(at, out, lock, cache, &pinned)
                .await
                .with_context(|| format!("failed to lock `{id}`"))?;
            Ok(((id, entry), deps))
        }))
        .then(identity)
        .try_fold(Lock::default(), |mut lock, ((id, entry), deps)| async {
            use std::collections::btree_map::Entry::{Occupied, Vacant};

            match lock.entry(id) {
                Occupied(e) => {
                    error!("duplicate lock entry for direct dependency `{}`", e.key());
                }
                Vacant(e) => {
                    trace!("record lock entry for direct dependency `{}`", e.key());
                    e.insert(entry);
                }
            }
            for (id, entry) in deps {
                match lock.entry(id) {
                    Occupied(e) => {
                        let other = e.get();
                        debug_assert!(other.source.is_none());
                        ensure!(other.digest == entry.digest, "transitive dependency conflict for `{}`, add `{}` to dependency manifest to resolve it", e.key(), e.key());
                        trace!(
                            "transitive dependency on `{}` already locked, skip",
                            e.key()
                        );
                    }
                    Vacant(e) => {
                        trace!("record lock entry for transitive dependency `{}`", e.key());
                        e.insert(entry);
                    }
                }
            }
            Ok(lock)
        })
        .await
    }
}

impl Deref for Manifest {
    type Target = HashMap<Identifier, Entry>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromIterator<(Identifier, Entry)> for Manifest {
    fn from_iter<T: IntoIterator<Item = (Identifier, Entry)>>(iter: T) -> Self {
        Self(HashMap::from_iter(iter))
    }
}

impl<const N: usize> From<[(Identifier, Entry); N]> for Manifest {
    fn from(entries: [(Identifier, Entry); N]) -> Self {
        Self::from_iter(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOO_URL: &str = "https://example.com/foo.tar.gz";

    const BAR_URL: &str = "https://example.com/bar";
    const BAR_SHA256: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    const BAZ_URL: &str = "http://127.0.0.1/baz";
    const BAZ_SHA256: &str = "9f86d081884c7d658a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    const BAZ_SHA512: &str = "ee26b0dd4af7e749aa1a8ee3c10ae9923f618980772e473f8819a5d4940e0db27ac185f8a0e1d5f84f88bc887fd67b143732c304cc5fa9ad8e6f57f50028a8ff";

    #[test]
    fn decode_url() -> anyhow::Result<()> {
        let manifest: Manifest = toml::from_str(&format!(
            r#"
foo = "{FOO_URL}"
bar = {{ url = "{BAR_URL}", sha256 = "{BAR_SHA256}" }}
baz = {{ url = "{BAZ_URL}", sha256 = "{BAZ_SHA256}", sha512 = "{BAZ_SHA512}" }}
"#
        ))
        .context("failed to decode manifest")?;
        assert_eq!(
            manifest,
            Manifest::from([
                (
                    "foo".parse().expect("failed to parse `foo` identifier"),
                    Entry::Url {
                        url: FOO_URL.parse().expect("failed to parse `foo` URL string"),
                        sha256: None,
                        sha512: None,
                        subdir: "wit".into(),
                    },
                ),
                (
                    "bar".parse().expect("failed to parse `bar` identifier"),
                    Entry::Url {
                        url: BAR_URL.parse().expect("failed to parse `bar` URL"),
                        sha256: FromHex::from_hex(BAR_SHA256)
                            .map(Some)
                            .expect("failed to decode `bar` sha256"),
                        sha512: None,
                        subdir: "wit".into(),
                    }
                ),
                (
                    "baz".parse().expect("failed to `baz` parse identifier"),
                    Entry::Url {
                        url: BAZ_URL.parse().expect("failed to parse `baz` URL"),
                        sha256: FromHex::from_hex(BAZ_SHA256)
                            .map(Some)
                            .expect("failed to decode `baz` sha256"),
                        sha512: FromHex::from_hex(BAZ_SHA512)
                            .map(Some)
                            .expect("failed to decode `baz` sha512"),
                        subdir: "wit".into(),
                    }
                )
            ])
        );
        Ok(())
    }

    #[test]
    fn decode_path() -> anyhow::Result<()> {
        let manifest: Manifest = toml::from_str(
            r#"
foo = "/path/to/foo"
bar = { path = "./path/to/bar" }
"#,
        )
        .context("failed to decode manifest")?;
        assert_eq!(
            manifest,
            Manifest::from([
                (
                    "foo".parse().expect("failed to parse `foo` identifier"),
                    Entry::Path(PathBuf::from("/path/to/foo")),
                ),
                (
                    "bar".parse().expect("failed to parse `bar` identifier"),
                    Entry::Path(PathBuf::from("./path/to/bar")),
                ),
            ])
        );
        Ok(())
    }
}
