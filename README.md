# Description

`wit-deps` is a simple WIT dependency manager binary and Rust library, which manages your `wit/deps`. It's main objective is to ensure that whatever is located in your `wit/deps` is consistent with your dependency manifest (default: `wit/deps.toml`) and dependency lock (default: `wit/deps.lock`).

# Manifest

A dependency manifest is a TOML-encoded table mapping dependency names to their source specifications. In it's simplest form, a source specification is a URL string of a gzipped tarball containing a directory tree with a `wit` subdirectory containing `wit` files.

Example:

```toml
# wit/deps.toml
# Use `wit-deps update` to pull in latest changes from "dynamic" branch references
clocks = "https://github.com/WebAssembly/wasi-clocks/archive/main.tar.gz"
http = "https://github.com/WebAssembly/wasi-http/archive/main.tar.gz"
messaging = "https://github.com/WebAssembly/wasi-messaging/archive/main.tar.gz"
sockets = "https://github.com/WebAssembly/wasi-sockets/archive/main.tar.gz"
sql = "https://github.com/WebAssembly/wasi-sql/archive/main.tar.gz"

# Pin to a tag
io = "https://github.com/rvolosatovs/wasi-io/archive/v0.1.0.tar.gz" # this fork renames `streams` interface for compatiblity with wasi-snapshot-preview1

# Pin a dependency to a particular revision and source digests. Each digest is optional
[keyvalue]
url = "https://github.com/WebAssembly/wasi-keyvalue/archive/6f3bd6bca07cb7b25703a13f633e05258d56a2dc.tar.gz"
sha256 = "1755b8f1e9f2e70d0bde06198bf50d12603b454b52bf1f59064c1877baa33dff"
sha512 = "7bc43665a9de73ec7bef075e32f67ed0ebab04a1e47879f9328e8e52edfb35359512c899ab8a52240fecd0c53ff9c036abefe549e5fb99225518a2e0446d66e0"

```

A source specfication can also be a structure with the following fields:

- `url` - same format as the URL string
- `sha256` - (optional) hex-encoded sha256 digest of the contents of the URL
- `sha512` (optional) hex-encoded sha512 digest of the contents of the URL
- `path` path to the directory containing the WIT definitions

Either `url` or `path` must be specified (both support string format)

Example:

```toml
# wit/deps.toml
mywit = "./path/to/my/wit"

[logging]
url = "https://github.com/WebAssembly/wasi-logging/archive/d106e59b25297d0496e6a5d221ad090e19c3aaa3.tar.gz"
sha256 = "4bb4aeab99e7323b30d107aab78e88b2265c1598cc438bc5fbc0d16bb63e798f"
sha512 = "13b52b59afd98dd4938e3a651fad631d41a2e84ce781df5d8957eded77a8e1ac4277e771a10225cd4a3a9eae369ed7e8fee6e26f9991a2caa7c97c4a758b1ae6"
```

# Usage

Note, `wit-deps` assumes that it has full control over `wit/deps` and so it may delete and modify contents of `wit/deps` at any time!

## Interactive

Use `wit-deps` or `wit-deps lock` to populate `wit/deps` using  `wit/deps.toml` manifest and `wit/deps.lock` (will be created if it does not exist)

To you it with a proxy, use the below environment variables:
```
export PROXY_SERVER={yourproxyaddress}:{port}
export PROXY_USERNAME='{yourproxyusername}'
export PROXY_PASSWORD='{yourproxypassword}'
```

## Rust

Use `wit-deps::lock!` macro in `build.rs` of your project to automatically lock your `wit/deps`.

See crate documentation for more advanced use cases

# Design decisions

- `wit-deps` is lazy by default and will only fetch/write when it absolutely has to
- `wit-deps` assumes that result of fetching from a URL is deterministic, that is contents returned by GET of a URL `domain.com` must always return exactly the same contents. Note, that you can use `sha256` or `sha512` fields in your manifest entry to invalidate the cache in this case
