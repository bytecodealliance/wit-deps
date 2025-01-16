# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2025-01-16

### Added

- URL dependencies now have support for `subdir` attribute, which overrides the default of `wit`.
  This feature is particularly useful for working with wasip3 draft packages

## [0.3.0] - 2023-04-11

### Added

- Transitive dependencies will now be pulled in from `wit/deps` of dependencies in the manifest

### Fixed

- Relative manifest path support in `wit-deps` binary

### Removed

- `package` argument to binary and library update and lock functions

## [0.2.2] - 2023-04-11

### Added

- `wit-deps update` along with the `wit-deps::update_path` and `wit-deps::update` library API

## [0.2.1] - 2023-04-10

### Fixed

- Ensure `path` in `deps.lock` matches the manifest `path`

## [0.2.0] - 2023-04-10

### Added

- Functionality to specify a path to `wit` directory in `lock!`
- `lock_sync!` macro executing `lock!` in a multi-threaded Tokio context. This macro is guarded by `sync` feature, which is enabled by default
- Support for path dependencies in `deps.toml`

## [0.1.0] - 2023-04-07

### Added

- Initial `wit-deps` library and binary implementations

[unreleased]: https://github.com/bytecodealliance/wit-deps/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/bytecodealliance/wit-deps/releases/tag/v0.5.0
[0.3.0]: https://github.com/bytecodealliance/wit-deps/releases/tag/v0.3.0
[0.2.2]: https://github.com/bytecodealliance/wit-deps/releases/tag/v0.2.2
[0.2.1]: https://github.com/bytecodealliance/wit-deps/releases/tag/v0.2.1
[0.2.0]: https://github.com/bytecodealliance/wit-deps/releases/tag/v0.2.0
[0.1.0]: https://github.com/bytecodealliance/wit-deps/releases/tag/v0.1.0
