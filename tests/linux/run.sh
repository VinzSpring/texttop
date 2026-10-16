#!/bin/sh
# Entry point of the Linux test container (see tests/linux/Dockerfile).
set -eu

echo "── unit + golden render tests ──"
# Golden tests compare against the snapshots in tests/golden; the browser
# engine needs a display even when no window is shown.
xvfb-run -a cargo test --release

echo "── end-to-end: drive texttop in a pseudo-terminal ──"
xvfb-run -a dbus-launch --exit-with-session \
    python3 tests/linux/e2e.py target/release/texttop "${E2E_OUT:-/out}"
