#!/usr/bin/env bash
# Make the warmed probe clones scripts/test-repo.sh copies from. Network once;
# the fixtures themselves never touch the network. Pinned so every run sees
# the same code: fd and sveltestrap at the shallow head of the day this was
# written (2026-09-02), ripgrep at 14.1.1 (master needs a newer rustc).
set -eu
P=/tmp/nebo-test/probe; mkdir -p "$P"; cd "$P"
[ -d fd ] || git clone -q --depth 1 https://github.com/sharkdp/fd fd
[ -d ripgrep ] || git clone -q --depth 1 --branch 14.1.1 https://github.com/BurntSushi/ripgrep ripgrep
[ -d sveltestrap ] || git clone -q --depth 1 https://github.com/sveltestrap/sveltestrap sveltestrap
(cd fd && cargo test -q test_size > /dev/null 2>&1) && echo "fd warmed"
(cd ripgrep && cargo test -q --bin rg flags::defs::test_max_count > /dev/null 2>&1) && echo "ripgrep warmed"
echo "probes ready under $P"
