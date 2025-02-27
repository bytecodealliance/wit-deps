use crate::{tar, Digest, DigestWriter, Identifier};

use core::ops::{Deref, DerefMut};

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use futures::io::sink;
use serde::{Deserialize, Serialize};
use url::Url;

fn default_subdir() -> Box<str> {
    "wit".into()
}

fn is_default_subdir(s: &str) -> bool {
    s == "wit"
}

/// Source of this dependency
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(untagged)]
pub enum EntrySource {
    /// URL
    Url {
        /// URL
        url: Url,
        /// Subdirectory containing WIT definitions within the tarball
        #[serde(default = "default_subdir", skip_serializing_if = "is_default_subdir")]
        subdir: Box<str>,
    },
    /// Local path
    Path {
        /// Local path
        path: PathBuf,
    },
}

/// WIT dependency [Lock] entry
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Entry {
    /// Resource source, [None] if the dependency is transitive
    #[serde(flatten)]
    pub source: Option<EntrySource>,
    /// Resource digest
    #[serde(flatten)]
    pub digest: Digest,
    /// Transitive dependency identifiers
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub deps: BTreeSet<Identifier>,
}

impl Entry {
    /// Create a new entry given a dependency source and path containing it
    #[must_use]
    pub fn new(source: Option<EntrySource>, digest: Digest, deps: BTreeSet<Identifier>) -> Self {
        Self {
            source,
            digest,
            deps,
        }
    }

    /// Create a new entry given a dependency url and path containing the unpacked contents of it
    ///
    /// # Errors
    ///
    /// Returns an error if [`Self::digest`] of `path` fails
    pub async fn from_url(
        url: Url,
        path: impl AsRef<Path>,
        deps: BTreeSet<Identifier>,
        subdir: impl Into<Box<str>>,
    ) -> anyhow::Result<Self> {
        let digest = Self::digest(path)
            .await
            .context("failed to compute digest")?;
        Ok(Self::new(
            Some(EntrySource::Url {
                url,
                subdir: subdir.into(),
            }),
            digest,
            deps,
        ))
    }

    /// Create a new entry given a dependency path
    ///
    /// # Errors
    ///
    /// Returns an error if [`Self::digest`] of `path` fails
    pub async fn from_path(
        src: PathBuf,
        dst: impl AsRef<Path>,
        deps: BTreeSet<Identifier>,
    ) -> anyhow::Result<Self> {
        let digest = Self::digest(dst)
            .await
            .context("failed to compute digest")?;
        Ok(Self::new(
            Some(EntrySource::Path { path: src }),
            digest,
            deps,
        ))
    }

    /// Create a new entry given a transitive dependency path
    ///
    /// # Errors
    ///
    /// Returns an error if [`Self::digest`] of `path` fails
    pub async fn from_transitive_path(dst: impl AsRef<Path>) -> anyhow::Result<Self> {
        let digest = Self::digest(dst)
            .await
            .context("failed to compute digest")?;
        Ok(Self::new(None, digest, BTreeSet::default()))
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

impl DerefMut for Lock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
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
                        source: Some(EntrySource::Url {
                            url: FOO_URL.parse().expect("failed to parse `foo` URL"),
                            subdir: "wit".into(),
                        }),
                        digest: Digest {
                            sha256: FromHex::from_hex(FOO_SHA256)
                                .expect("failed to decode `foo` sha256"),
                            sha512: FromHex::from_hex(FOO_SHA512)
                                .expect("failed to decode `foo` sha512"),
                        },
                        deps: BTreeSet::default(),
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
