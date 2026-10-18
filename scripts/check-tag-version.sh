#!/usr/bin/env bash
# A release tag names the version its commit builds. v0.16.20 was first
# tagged on main before the version bump: its binaries said 0.16.16 and its
# server image moved the cloud fleet. Tag the release branch's bump commit.
set -euo pipefail
version=$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)
if [ "v$version" != "$1" ]; then
  echo "::error::tag $1, but Cargo.toml says $version: tag the release branch's version bump commit"
  exit 1
fi
echo "tag $1 matches Cargo.toml $version"
