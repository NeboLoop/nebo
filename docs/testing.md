# Testing: what runs where

One change is tested once at each gate, never the same suite over and over.
The Rust tests run under [cargo-nextest](https://nexte.st): every test in a
process of its own, many side by side. The configuration is
`.config/nextest.toml`; the CI wiring is `.github/workflows/ci.yml`.

## The tiers

| When | What runs | Where |
|------|-----------|-------|
| Pull request | The crates the diff touches, every crate that depends on them, and the tree guards (`scripts/ci-test-crates.py`). The end-to-end proofs only when the diff touches them. | `ci.yml`, GitHub-hosted (the repo is public) |
| Push to main | Builds and lists every test binary: the merge still compiles and every binary starts. No tests: they ran on the pull request's merge ref. | `ci.yml` |
| Nightly (09:17 UTC) | Every test, the end-to-end proofs included, and the doctests. | `ci.yml` `schedule` (and `workflow_dispatch` to run it by hand) |
| Release gate | The nightly run, on the commit being tagged. | `release.yml` (added after v0.16.18) |
| Your machine | What you changed: `cargo nextest run -p <crate>`. 4 tests at a time. | `.config/nextest.toml` `[profile.default]` |

The frontend job (`pnpm lint`, `pnpm test`) runs on every pull request and
push; it takes about a minute.

## The end-to-end proofs

These boot a real server, or wait on real engine ticks and sockets. They are
the slow third of the suite, so a pull request runs them only when it
touches them (`PROOF_INPUTS` in `scripts/ci-test-crates.py`: the proofs'
own files, the server's engine and scheduler, its integration tests, their
suites and fixtures). The nightly and the release gate always run them.

- `nebo-server` `staffed_proof::*::*` — the staffed company, one scenario per
  test on a real server (`suites/staffed-company.yaml`, `suites/memory-proof.yaml`)
- `nebo-server` `harness::permissions::proof::*`
- `nebo-server` integration tests `unattended_work`, `drain`, `cross_device`,
  `app_record`, `temporary_teams`, `long_running_work`

The set is the `default-filter` in `.config/nextest.toml`. To run them:

```bash
cargo nextest run -p nebo-server --profile full   # this crate, proofs included
make test-full                                    # everything (CI's job; not on a shared Mac)
```

## Flaky tests: the quarantine

A test that fails now and then goes in the quarantine at the bottom of
`.config/nextest.toml`, with its issue: it is retried, and a pass on a retry
is reported as FLAKY without failing the run. A test that fails every attempt
still fails the run. The entry leaves when its issue closes. A test that
simply fails is fixed, never quarantined.

## Writing a test that stays fast

- Never sleep for a fixed time and hope. Wait on the event (a queue length,
  a status, a row), with a timeout that only a broken build reaches.
- Time the product measures in seconds is time the test should not spend:
  `#[tokio::test(start_paused = true)]` with `tokio::time::advance`, or an
  injected clock or interval.
- Process-wide state (`OnceLock`s, the environment, the shared proof server)
  is per test under nextest. A test that passes only after another one set
  something up in the same process is broken; set it up in the test.

## Doctests

nextest does not run doctests. The nightly and the release gate run
`cargo test --doc` for the whole workspace; a pull request does not (the
workspace has one doctest, in `nebo-command`, and building every selected
crate's docs for it cost each pull request a minute).
