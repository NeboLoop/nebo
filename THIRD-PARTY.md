# Third-Party Notices

Nebo is licensed under the MIT License. It bundles and vendors third-party
components that retain their own licenses, reproduced or referenced below.

Rust and npm dependencies resolved via `Cargo.lock` and `pnpm-lock.yaml` are
permissively licensed (MIT / Apache-2.0 / BSD / ISC); their license texts are
distributed with the compiled artifacts. The npm dependency licenses are also
aggregated in `app/static/LICENSES.txt`.

## Bundled binaries

### Obscura

Nebo bundles the Obscura headless browser (`src-tauri/binaries/obscura*`), used
for browser automation.

- Upstream: https://github.com/h4ckf0r0day/obscura (https://obscura.sh)
- License: Apache License 2.0
- Nebo builds these binaries from a fork (`localrivet/obscura`) with
  CDP-compatibility modifications; the bundled binaries are modified versions
  of the upstream source.

The Apache-2.0 license text and any upstream NOTICE apply to these binaries and
are retained with them.

## Vendored source

### a2ui-rs

`crates/a2ui/` vendors the a2ui-rs A2UI protocol toolkit.

- Author: AppleGrew
- License: MIT (see `crates/a2ui/LICENSE`)
