use core::fmt;
use core::ops::{Deref, DerefMut};

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _};
use async_trait::async_trait;
use directories::ProjectDirs;
use futures::{io::BufReader, AsyncBufRead, AsyncWrite};
use tokio::fs::{self, File, OpenOptions};
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};
use url::{Host, Url};

/// Resource caching layer
#[async_trait]
pub trait Cache {
    /// Type returned by the [Self::get] method
    type Read: AsyncBufRead + Unpin;
    /// Type returned by the [Self::insert] method
    type Write: AsyncWrite + Unpin;

    /// Returns a read handle for the entry from the cache associated with a given URL
    async fn get(&self, url: &Url) -> anyhow::Result<Option<Self::Read>>;

    /// Returns a write handle for the entry associated with a given URL
    async fn insert(&self, url: &Url) -> anyhow::Result<Self::Write>;
}

/// Write-only [Cache] wrapper
pub struct Write<T>(pub T);

impl<T> From<T> for Write<T> {
    fn from(cache: T) -> Self {
        Self(cache)
    }
}

impl<T> Deref for Write<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Write<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[async_trait]
impl<T: Cache + Sync + Send> Cache for Write<T> {
    type Read = T::Read;
    type Write = T::Write;

    async fn get(&self, _: &Url) -> anyhow::Result<Option<Self::Read>> {
        Ok(None)
    }

    async fn insert(&self, url: &Url) -> anyhow::Result<Self::Write> {
        self.0.insert(url).await
    }
}

impl<T> Write<T> {
    /// Extracts the inner [Cache]
    pub fn into_inner(self) -> T {
        self.0
    }
}

/// Local caching layer
#[derive(Clone, Debug)]
pub struct Local(PathBuf);

impl fmt::Display for Local {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl Deref for Local {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Local {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Local {
    /// Returns a [Local] cache located at the default system-specific cache directory if such
    /// could be determined.
    pub fn cache_dir() -> Option<Self> {
        ProjectDirs::from("", "", env!("CARGO_PKG_NAME"))
            .as_ref()
            .map(ProjectDirs::cache_dir)
            .map(Self::from)
    }

    fn path(&self, url: &Url) -> impl AsRef<Path> {
        let mut path = self.0.clone();
        match url.host() {
            Some(Host::Ipv4(ip)) => {
                path.push(ip.to_string());
            }
            Some(Host::Ipv6(ip)) => {
                path.push(ip.to_string());
            }
            Some(Host::Domain(domain)) => {
                path.push(domain);
            }
            _ => {}
        }
        if let Some(segments) = url.path_segments() {
            for seg in segments {
                path.push(seg);
            }
        }
        path
    }
}

#[async_trait]
impl Cache for Local {
    type Read = BufReader<Compat<File>>;
    type Write = Compat<File>;

    async fn get(&self, url: &Url) -> anyhow::Result<Option<Self::Read>> {
        match File::open(self.path(url)).await {
            Ok(file) => Ok(Some(BufReader::new(file.compat()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => bail!("failed to lookup `{url}` in cache: {e}"),
        }
    }

    async fn insert(&self, url: &Url) -> anyhow::Result<Self::Write> {
        let path = self.path(url);
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)
                .await
                .context("failed to create directory")?;
        }
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .await
            .map(tokio_util::compat::TokioAsyncReadCompatExt::compat)
            .context("failed to open file for writing")
    }
}

impl From<PathBuf> for Local {
    fn from(path: PathBuf) -> Self {
        Self(path)
    }
}

impl From<String> for Local {
    fn from(path: String) -> Self {
        Self(path.into())
    }
}

impl From<OsString> for Local {
    fn from(path: OsString) -> Self {
        Self(path.into())
    }
}

impl From<&Path> for Local {
    fn from(path: &Path) -> Self {
        Self(path.into())
    }
}

impl From<&str> for Local {
    fn from(path: &str) -> Self {
        Self(path.into())
    }
}

impl From<&OsStr> for Local {
    fn from(path: &OsStr) -> Self {
        Self(path.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_path() {
        assert_eq!(
            Local::from("test")
                .path(
                    &"https://example.com/foo/bar.tar.gz"
                        .parse()
                        .expect("failed to parse URL")
                )
                .as_ref(),
            Path::new("test")
                .join("example.com")
                .join("foo")
                .join("bar.tar.gz")
        );
    }
}
