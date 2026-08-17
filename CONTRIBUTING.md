# Contributing

Thanks for taking a look. This document covers how to get set up, how to run the same checks CI
runs, and what a change has to include to get merged.

## Setup

| Requirement                                | Why                                                                                                    |
|--------------------------------------------|--------------------------------------------------------------------------------------------------------|
| Rust stable                                | The crate is edition 2024, so 1.85 or newer                                                            |
| [`protoc`][protoc]                         | Every example and `integration/` runs `tonic-prost-build` in a `build.rs`, and prost-build no longer vendors a protoc |
| [`cargo-lambda`][cargo-lambda]             | Only for running anything against a real lambda — not needed for the unit tests                        |
| A `.env` in the repo root                  | See [network tests](#network-tests) below                                                              |

[protoc]: https://protobuf.dev/installation/
[cargo-lambda]: https://cargo-lambda.info

## Layout

There are **two** cargo workspaces:

| Path                        | What it is                                                              |
|-----------------------------|-------------------------------------------------------------------------|
| `.`                         | The `lambda-grpc-web` library — the only crate that gets published       |
| `integration/`              | A service plus tests that drive it over the wire                        |
| `examples/hello-world/`     | Minimal unary example                                                   |
| `examples/server-streaming/`| Server streaming example                                                |
| `examples/envoy/`           | Envoy example — **its own workspace**                                   |

`examples/envoy` is excluded from the root workspace on purpose. Cargo unifies features across the
members it builds together, and that example selects `transport-envoy` while everything else
selects `transport-apigw-http` — as one workspace, both transports would end up enabled at once and
trip the `compile_error!` guard. The practical consequence is that **every command has to be run
twice**, once per workspace.

## Working with the transport features

The library will not compile without exactly one transport feature, and will not compile with both
(see the `compile_error!` calls in `src/lib.rs`). So:

* `cargo check -p lambda-grpc-web` on its own **fails** — that is the guard working, not a broken
  checkout. Pass `--features transport-apigw-http` or `--features transport-envoy`.
* `--all-features` **never** works on this crate. Anything that needs full coverage runs once per
  transport instead.

## Running the checks

These are exactly what CI runs, in the same order. All of them must pass.

```shell
# formatting, both workspaces
cargo fmt --all --check
cargo fmt --all --check --manifest-path examples/envoy/Cargo.toml

# clippy, once per transport for the library, plus the other crates
cargo clippy --locked --workspace --all-targets --features transport-apigw-http -- -D warnings
cargo clippy --locked -p lambda-grpc-web --all-targets --features transport-apigw-http,wire-log -- -D warnings
cargo clippy --locked -p lambda-grpc-web --all-targets --features transport-envoy,wire-log -- -D warnings
cargo clippy --locked --all-targets --manifest-path examples/envoy/Cargo.toml -- -D warnings

# unit tests, once per transport
cargo test --locked -p lambda-grpc-web --features transport-apigw-http,wire-log
cargo test --locked -p lambda-grpc-web --features transport-envoy,wire-log
```

`cargo fmt --all` (in both workspaces) fixes most formatting complaints.

## Tests

### Unit tests

Everything under `cargo test -p lambda-grpc-web` is self contained and is what CI gates on. Note
that today all of these live in `src/transport/envoy.rs` — `src/transport/apigw_http.rs` has no
unit tests yet, so a change there is worth covering.

The tests that assert on log output share a global capture buffer. Records are tagged with the
thread that emitted them so that a test only ever reads its own output; if you add a test that
asserts a warning did *not* fire, go through `logs_since` rather than reading the buffer directly.

### Network tests

The tests in `integration/` and in each example dial a real lambda, so they need one running and
**CI never executes them** — the clippy pass is what keeps them compiling. To run them yourself:

```shell
cargo lambda watch -p integration     # terminal 1
cargo test -p integration             # terminal 2
```

`integration` reads its target address through `dotenvy_macro::dotenv!`, which resolves **at
compile time**, so a `.env` has to exist in the repo root before the crate will even build:

```shell
ORIGIN_URI=http://0.0.0.0:9000
```

It is gitignored, which keeps unsecured function URLs out of the repository. CI writes a
placeholder so the crate can be linted; nothing there ever connects to it.

The envoy example is invoked rather than served, so it works a little differently:

```shell
cd examples/envoy
cargo lambda watch                                                    # terminal 1
cargo lambda invoke example-envoy --data-file events/say_hello.json   # terminal 2
```

## Making a change

1. Branch off `master`.
2. Make the change, with tests where the behaviour is testable without a live lambda.
3. **Bump `version` in the root `Cargo.toml`** — see below.
4. Run [the checks](#running-the-checks).
5. Open a PR.

### Version bumps

Merging to `master` publishes to crates.io, so a PR is expected to bump the version and CI enforces
it. Use semver: the transport features and `LambdaServer` are public API.

If a change should not cut a release — docs, CI, a test only fix — add the **`no-release`** label to
the PR and the check goes green. The label is picked up as soon as you add it; no need to push
again.

> [!IMPORTANT]
> Bumping the version leaves **both** lockfiles stale, because each records the version of
> `lambda-grpc-web` itself. CI builds with `--locked` and will fail until they are refreshed and
> committed:
>
> ```shell
> cargo check --features transport-apigw-http
> cargo check --manifest-path examples/envoy/Cargo.toml
> git add Cargo.lock examples/envoy/Cargo.lock
> ```

## Releasing

Releases are automatic — there is no manual `cargo publish` step. On a push to `master`, once
rustfmt, clippy and the tests have passed, the publish job asks crates.io whether the version in
`Cargo.toml` already exists and publishes it if not. Because it compares against the registry
rather than against the previous commit, re-running is harmless and a release missed by a failed run
is picked up by the next push.

Two things about that job are load bearing if you ever need to change it:

* It publishes with `--features transport-apigw-http`. `cargo publish` verifies by building the
  packaged crate, and that build would hit the "no transport" guard under the default features. The
  flag only picks which transport gets compiled during verification — the published crate still
  offers both.
* It authenticates over OIDC ([trusted publishing][trusted-publishing]), so there is no crates.io
  token stored in repository secrets. This has to be registered once on the crates.io side, against
  this repository and the `CI` workflow.

docs.rs has the same "no transport" problem and is handled by `[package.metadata.docs.rs]` in
`Cargo.toml`, which pins the docs build to `transport-envoy` — the two transports expose the same
API apart from the `EnvoyRequest`/`EnvoyResponse` envelopes, so that setting documents the superset.
docs.rs only rebuilds on publish, so a fix there takes effect on the next release.

[trusted-publishing]: https://github.com/rust-lang/crates-io-auth-action

## A note on the tonic patch

The root `Cargo.toml` and `examples/envoy/Cargo.toml` both carry a `[patch.crates-io]` pointing
`tonic` and `tonic-web` at a fork, pending [hyperium/tonic#2474][tonic-pr]. Patches apply to local
builds only — they are ignored for a crate consumed from crates.io — so published releases resolve
against upstream. If you touch it, keep the two copies in step; the example needs its own because it
is a separate workspace root.

[tonic-pr]: https://github.com/hyperium/tonic/pull/2474
