# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.2] - 2023-04-11

### Added

- `depit update` along with the `depit::update_path` and `depit::update` library API

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

- Initial `depit` library and binary implementations

[unreleased]: https://github.com/rvolosatovs/depit/compare/v0.2.2...HEAD
[0.2.2]: https://github.com/rvolosatovs/depit/releases/tag/v0.2.2
[0.2.1]: https://github.com/rvolosatovs/depit/releases/tag/v0.2.1
[0.2.0]: https://github.com/rvolosatovs/depit/releases/tag/v0.2.0
[0.1.0]: https://github.com/rvolosatovs/depit/releases/tag/v0.1.0
