# Changelog

All notable changes to this fork are documented here. This project is a fork of
[delta62/mds](https://github.com/delta62/mds); versions up to and including
`1.0.0` are inherited from upstream.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.1] - 2026-05-19

### Added

- Per-platform installation guide in the README: artifact-picker table, the
  glibc-vs-musl choice for Linux, and per-OS extract/install steps covering the
  quirks that bite (macOS Gatekeeper quarantine, Windows SmartScreen + the VC++
  redistributable, the glibc version floor).

### Changed

- Release workflow rebuilt on `taiki-e/upload-rust-binary-action`, trimmed to
  the major architectures (x86_64 + aarch64) with clean artifact names
  (`mds-linux-x86_64`, `mds-macos-x86_64`, …) instead of raw target triples.
  macOS x86_64 now builds on the Apple Silicon runner so it stops getting
  cancelled. Bumped `actions/checkout` to v5.

### Fixed

- Release builds no longer fail to parse `Cargo.lock`: the previous
  `rust-build.action` shipped a Cargo too old for the lockfile v4 format, which
  broke every target.

## [1.1.0] - 2026-05-19

### Added

- `mds extract <FILE.mds>` — reads files directly out of an `.mdf` without
  producing an intermediate ISO.
  - `-o/--output <DIR>` to choose the destination (defaults to the `.mds`
    basename); `--list` to print the tree without writing; `--force` to extract
    into a non-empty directory.
  - Prefers Joliet (Unicode) names via the supplementary volume descriptor,
    falling back to primary ISO9660 names. Strips `;1` version suffixes.
  - Hand-rolled ISO9660 reader (no new dependencies) hardened against malformed
    images: bounded directory-extent allocation, recursion depth + ancestor
    cycle detection, both-endian field validation, sector-boundary checks,
    multi-extent rejection, and even-length Joliet identifier validation.
  - Path-traversal and symlink defences on the output side, plus control-char
    escaping in `--list` output.
- `CookedSectorReader`, a 2048-byte logical-sector view over raw 2352/2336/2448
  track sectors, shared with the existing ISO conversion path.

### Changed

- ISO conversion (`convert --format iso`) carries fork fixes over upstream
  `1.0.0`: extracts cooked ISO user-data from raw-mode tracks, handles the
  MODE2/2336 (0x920) layout, and seeks to the track start offset before
  reading. `iso_user_data_range` now lives in the shared `cooked` module so
  conversion and extraction share one source of truth.

[Unreleased]: https://github.com/pacnpal/mds/compare/v1.1.1...HEAD
[1.1.1]: https://github.com/pacnpal/mds/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/pacnpal/mds/releases/tag/v1.1.0
