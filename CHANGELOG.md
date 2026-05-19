# Changelog

All notable changes to this fork are documented here. This project is a fork of
[delta62/mds](https://github.com/delta62/mds); versions up to and including
`1.0.0` are inherited from upstream.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/pacnpal/mds/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/pacnpal/mds/releases/tag/v1.1.0
