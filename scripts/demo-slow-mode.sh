#!/bin/sh
# Local walkthrough of /slow-mode. Nothing here calls the OpenAI API.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)

cat <<'EOF'
codex

> /slow-mode
Slow mode enabled for this session.

> Build the feature...
# model requests wait in ModelClientSession::stream before they are sent
# shell, tests, and file reads are not given an extra delay

> /slow-mode status
Slow mode: ON
Fast mode: OFF

> /fast on
Fast mode stays off while Slow mode is on. Run /slow-mode off first.

> /slow-mode off
Slow mode disabled for this session.
EOF

echo
echo "Deterministic pacing report:"
(
  cd "$ROOT/codex-rs"
  cargo run -q -p codex-slow-mode --bin slow-mode-simulate
)
