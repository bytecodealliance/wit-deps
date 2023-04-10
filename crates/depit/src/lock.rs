use crate::{tar, Digest, DigestWriter, Identifier};

use core::ops::Deref;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use futures::io::sink;
use serde::{Deserialize, Serialize};
use url::Url;

/// Source of this dependency
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum EntrySource {
    /// URL
    #[serde(rename = "url")]
    Url(Url),
    /// Local path
    #[serde(rename = "path")]
    Path(PathBuf),
}

/// WIT dependency [Lock] entry
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Entry {
    /// Resource source
    #[serde(flatten)]
    pub source: EntrySource,
    /// Resource digest
    #[serde(flatten)]
    pub digest: Digest,
}

impl Entry {
    /// Create a new entry given a dependency source and path containing it
    #[must_use]
    pub fn new(source: EntrySource, digest: Digest) -> Self {
        Self { source, digest }
    }

    /// Create a new entry given a dependency url and path containing the unpacked contents of it
    ///
    /// # Errors
    ///
    /// Returns an error if [`Self::digest`] of `path` fails
    pub async fn from_url(url: Url, path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let digest = Self::digest(path)
            .await
            .context("failed to compute digest")?;
        Ok(Self::new(EntrySource::Url(url), digest))
    }

    /// Create a new entry given a dependency path
    ///
    /// # Errors
    ///
    /// Returns an error if [`Self::digest`] of `path` fails
    pub async fn from_path(src: PathBuf, dst: impl AsRef<Path>) -> anyhow::Result<Self> {
        let digest = Self::digest(dst)
            .await
            .context("failed to compute digest")?;
        Ok(Self::new(EntrySource::Path(src), digest))
    }

    /// Compute the digest of an entry from path
    ///
    /// # Errors
    ///
    /// Returns an error if tar-encoding the path fails
    pub async fn digest(path: impl AsRef<Path>) -> std::io::Result<Digest> {
        tar(path, DigestWriter::from(sink())).await.map(Into::into)
    }
}

/// WIT dependency lock mapping [Identifiers](Identifier) to [Entries](Entry)
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Lock(BTreeMap<Identifier, Entry>);

impl Deref for Lock {
    type Target = BTreeMap<Identifier, Entry>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromIterator<(Identifier, Entry)> for Lock {
    fn from_iter<T: IntoIterator<Item = (Identifier, Entry)>>(iter: T) -> Self {
        Self(BTreeMap::from_iter(iter))
    }
}

impl Extend<(Identifier, Entry)> for Lock {
    fn extend<T: IntoIterator<Item = (Identifier, Entry)>>(&mut self, iter: T) {
        self.0.extend(iter);
    }
}

impl<const N: usize> From<[(Identifier, Entry); N]> for Lock {
    fn from(entries: [(Identifier, Entry); N]) -> Self {
        Self::from_iter(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use anyhow::{ensure, Context};
    use hex::FromHex;

    const FOO_URL: &str = "https://example.com/baz";
    const FOO_SHA256: &str = "9f86d081884c7d658a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    const FOO_SHA512: &str = "ee26b0dd4af7e749aa1a8ee3c10ae9923f618980772e473f8819a5d4940e0db27ac185f8a0e1d5f84f88bc887fd67b143732c304cc5fa9ad8e6f57f50028a8ff";

    #[test]
    fn decode() -> anyhow::Result<()> {
        fn assert_lock(lock: Lock) -> anyhow::Result<Lock> {
            ensure!(
                lock == Lock::from([(
                    "foo".parse().expect("failed to `foo` parse identifier"),
                    Entry {
                        source: EntrySource::Url(
                            FOO_URL.parse().expect("failed to parse `foo` URL")
                        ),
                        digest: Digest {
                            sha256: FromHex::from_hex(FOO_SHA256)
                                .expect("failed to decode `foo` sha256"),
                            sha512: FromHex::from_hex(FOO_SHA512)
                                .expect("failed to decode `foo` sha512"),
                        },
                    }
                )])
            );
            Ok(lock)
        }

        let lock = toml::from_str(&format!(
            r#"
foo = {{ url = "{FOO_URL}", sha256 = "{FOO_SHA256}", sha512 = "{FOO_SHA512}" }}
"#
        ))
        .context("failed to decode lock")
        .and_then(assert_lock)?;

        let lock = toml::to_string(&lock).context("failed to encode lock")?;
        toml::from_str(&lock)
            .context("failed to decode lock")
            .and_then(assert_lock)?;

        Ok(())
    }
}
