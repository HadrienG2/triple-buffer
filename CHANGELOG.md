# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).


## [Unreleased]

### Changed

- Turn `Input::input_buffer()` and `Output::output_buffer()` into read-only
  accessors and deprecate `Output::peek_output_buffer()`, moving forward with
  the plan set in issue #30 to eventually migrate towards an API naming
  convention that matches `std` and other Rust libraries.



## [8.1.1] - 2025-05-04

### Changed

- Switched to edition 2021 since the current MSRV allows for it

### Fixed

- Updated README to reflect current API
- Commit lockfile to avoid surprise semver/MSRV breakage from deps


## [8.1.0] - 2025-02-02

### Added

- Add `Input::input_buffer_publisher()` method to provide an RAII-based
  alternative to the low-level `input_buffer()`/`publish()` interface. Thanks
  @crop2000 !

### Changed

- Rename `Input::input_buffer()` to `Input::input_buffer_mut()`, keeping a
  deprecated alias for now, and do the same for `Output::output_buffer()`. This
  is the start of a gradual deprecation process whose end goal is to eventually
  follow the standard Rust accessor naming convention (`input_buffer(&self) ->
  &T`, `input_buffer_mut(&mut self) -> &mut T`, same thing on the output side).


## [8.0.0] - 2024-06-21

### Added

- Add `Output::peek_output_buffer()` method to get read-only access to the
  output buffer from a shared reference to self. Thanks @tk70 !

### Changed

- Bumped MSRV to 1.74 owing to new dependency requirements.
- Refactor CI workflow file to account for the latest GitHub CI oddities.


## [7.0.0] - 2023-10-22

### Changed

- Bumped MSRV to 1.70 owing to new dependency requirements.


## [6.2.0] - 2022-06-27

### Added

- A `triple_buffer()` shorthand is now available for the common
  `TripleBuffer::new().split()` pattern.

### Changed

- The documentation example now features multi-threading to clarify ownership.


## [6.1.0] - 2022-10-05

### Added

- `triple-buffer` is now usable in `no_std` contexts where an implementation of
  the `alloc` crate is available.


## [6.0.0] - 2021-12-18

### Changed

- Latest dependency versions require Rust 1.46, we bump MSRV accordingly.
- ...and since that's a breaking change, I'm also flushing the breaking change
  pipeline along the way:
    * TripleBuffer::new now takes a reference to its input.
    * The deprecated `raw` feature is now removed.


## [5.0.6] - 2021-01-16

### Added

- As a result of the bugfix mentioned below, there is no performance motivation
  to gate `raw` features behind a feature flag, so those features are now
  available by default without a `raw_` prefix. Usage of the `raw_` prefix and
  the `raw` feature flag is deprecated and these may be removed in a future
  major release, but it doesn't harm to keep them indefinitely for now.

### Changed

- Benchmarks now use `criterion`, and have been significantly cleaned up along
  the way. They are now more extensive and more reliable.
- Moved MSRV to Rust 1.36 because we now use crossbeam for testing, which
  requires that much. The crate itself should still support Rust 1.34 for now,
  but we cannot test that it continues doing so...

### Fixed

- Removed a possibility of data race that was not observed on current hardware,
  but could be triggered by future hardware or compiler evolutions. See
  https://github.com/HadrienG2/triple-buffer/issues/14 .


## [5.0.5] - 2020-07-05

### Changed

- Use only cache-padded instead of the full crossbeam-utils crate
- Clean up CI config and cache Rust toolchain there


## [5.0.4] - 2020-02-10

### Added

- Add a changelog to the repository.

### Changed

- Deduplicate CI configuration some more.

### Fixed

- Drop now-unnecessary manual `rustfmt` configuration.
- Avoid false sharing of back-buffer information.


## [5.0.3] - 2020-02-07

### Changed

- Clean up and deduplicate GitHub Actions configuration.
- Tune down concurrent test speed to reduce CI false positives.


## [5.0.2] - 2020-01-29

### Changed

- Move continuous integration to GitHub Actions.


## [5.0.1] - 2019-11-07

### Fixed

- Update to current version of dependencies.


## [5.0.0] - 2019-04-12

### Changed

- Bump travis CI configuration to Ubuntu Xenial.
- Bump minimal supported Rust version to 1.34.0.

### Fixed

- Don't use an `usize` for buffer indices where an `u8` will suffice.
- Improve Rust API guidelines compliance.


## [4.0.1] - 2018-12-31

### Fixed

- Display `raw` feature documentation on docs.rs.


## [4.0.0] - 2018-12-18

### Changed

- Migrate to Rust 2018.
- Bump minimal supported Rust version to 1.31.0.

### Fixed

- Update to current version of dependencies.
- Start using Clippy and integrate it into continuous integration.
- Re-apply `rustfmt` coding style (was not in CI at the time...).


## [3.0.1] - 2018-08-27

### Fixed

- Make `testbench` a dev-dependency, as it's only used for tests and benchmarks.


## [3.0.0] - 2018-08-27

### Changed

- Buffers are now padded to the size of a cache line to reduce false sharing.
- Bump minimal supported Rust version to 1.26.0.

### Fixed

- Make `testbench` version requirement more explicit.


## [2.0.0] - 2018-02-11

### Changed

- Switch license to MPLv2, which is a better match to Rust's static linking
  philosophy than LGPL.


## [1.1.1] - 2017-11-19

### Fixed

- Fix my understanding of Cargo features & make the `raw` feature actually work.


## [1.1.0] - 2017-11-18

### Added

- Allow in-place writes on the input and output side, at the cost of stronger
  synchronization barriers, through use of the `raw` Cargo feature.

### Fixed

- Do not require a `Clone` bound on the inner data.


## [1.0.0] - 2017-11-10

### Changed

- Simplify component naming convention, e.g. `TripleBufferInput` -> `Input`.


## [0.3.4] - 2017-06-25

### Changed

- Use `testbench::RaceCell` as an improved form of data race detection in tests.

### Fixed

- Do not require a `PartialEq` bound on the inner data.


## [0.3.3] - 2017-06-15

### Changed

- Tune down concurrent test speed to reduce CI false positives.


## [0.3.2] - 2017-06-15

### Changed

- Tune down concurrent test speed to reduce CI false positives.


## [0.3.1] - 2017-06-15

### Changed

- Tune down concurrent test speed to reduce CI false positives.


## [0.3.0] - 2017-06-14

### Added

- Introduce Travis CI continuous integration.

### Fixed

- Use CI to clarify minimal supported Rust version (currently 1.12.0).


## [0.2.4] - 2017-04-04

### Changed

- Use `testbench` crate for concurrent testing and benchmarking.


## [0.2.3] - 2017-03-24

### Changed

- More detailed comparison with other synchronization primitives in README.

### Fixed

- Adopt `rustfmt` coding style.


## [0.2.2] - 2017-03-20

### Changed

- Reduce reliance on Acquire-Release synchronization.


## [0.2.1] - 2017-03-11

### Changed

- Make README a bit more spambot-proof.


## [0.2.0] - 2017-03-11

### Added

- First tagged release of triple-buffer.



[Unreleased]: https://github.com/HadrienG2/triple-buffer/compare/v8.1.1...HEAD
[8.1.1]: https://github.com/HadrienG2/triple-buffer/compare/v8.1.0...v8.1.1
[8.1.0]: https://github.com/HadrienG2/triple-buffer/compare/v8.0.0...v8.1.0
[8.0.0]: https://github.com/HadrienG2/triple-buffer/compare/v7.0.0...v8.0.0
[7.0.0]: https://github.com/HadrienG2/triple-buffer/compare/v6.2.0...v7.0.0
[6.2.0]: https://github.com/HadrienG2/triple-buffer/compare/v6.1.0...v6.2.0
[6.1.0]: https://github.com/HadrienG2/triple-buffer/compare/v6.0.0...v6.1.0
[6.0.0]: https://github.com/HadrienG2/triple-buffer/compare/v5.0.6...v6.0.0
[5.0.6]: https://github.com/HadrienG2/triple-buffer/compare/v5.0.5...v5.0.6
[5.0.5]: https://github.com/HadrienG2/triple-buffer/compare/v5.0.4...v5.0.5
[5.0.4]: https://github.com/HadrienG2/triple-buffer/compare/v5.0.3...v5.0.4
[5.0.3]: https://github.com/HadrienG2/triple-buffer/compare/v5.0.2...v5.0.3
[5.0.2]: https://github.com/HadrienG2/triple-buffer/compare/v5.0.1...v5.0.2
[5.0.1]: https://github.com/HadrienG2/triple-buffer/compare/v5.0.0...v5.0.1
[5.0.0]: https://github.com/HadrienG2/triple-buffer/compare/v4.0.1...v5.0.0
[4.0.1]: https://github.com/HadrienG2/triple-buffer/compare/v4.0.0...v4.0.1
[4.0.0]: https://github.com/HadrienG2/triple-buffer/compare/v3.0.1...v4.0.0
[3.0.1]: https://github.com/HadrienG2/triple-buffer/compare/v3.0.0...v3.0.1
[3.0.0]: https://github.com/HadrienG2/triple-buffer/compare/v2.0.0...v3.0.0
[2.0.0]: https://github.com/HadrienG2/triple-buffer/compare/v1.1.1...v2.0.0
[1.1.1]: https://github.com/HadrienG2/triple-buffer/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/HadrienG2/triple-buffer/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/HadrienG2/triple-buffer/compare/v0.3.4...v1.0.0
[0.3.4]: https://github.com/HadrienG2/triple-buffer/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/HadrienG2/triple-buffer/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/HadrienG2/triple-buffer/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/HadrienG2/triple-buffer/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/HadrienG2/triple-buffer/compare/v0.2.4...v0.3.0
[0.2.4]: https://github.com/HadrienG2/triple-buffer/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/HadrienG2/triple-buffer/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/HadrienG2/triple-buffer/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/HadrienG2/triple-buffer/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/HadrienG2/triple-buffer/releases/tag/v0.2.0
