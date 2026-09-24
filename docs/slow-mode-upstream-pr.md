# Slow Mode for Codex

## Summary

- Add a session-scoped `/slow-mode` command that spaces model inference across the primary usage window instead of letting an autonomous task spend the allowance in a burst.
- Gate requests inside `ModelClientSession::stream`, before a Responses call is built, and hold one shared permit until that response stream ends. Shell, tests, compilation, file reads, and MCP tool execution are not delayed.
- Keep the model, reasoning effort, tools, and prompts unchanged. Fast mode is suspended for the session and restored on `/slow-mode off` without writing the saved service-tier preference.

## Why

A long Codex task issues many model calls close together: the agent turn, tool follow-ups, review, and subagents. The user is willing for that work to take longer so the five-hour window is consumed on a schedule that can last until reset.

## Behavior

- `/slow-mode` and `/slow-mode on` enable pacing for this session.
- `/slow-mode off` cancels pending waits and restores the previous session Fast tier when Slow Mode had changed it.
- `/slow-mode status` prints the local snapshot. None of these commands start a turn.
- Parallel model calls share one queue. Subagents keep running; their model calls wait their turn.
- The controller uses `used_percent`, window length, and `resets_at` for the primary and secondary windows, with an EWMA of observed request cost and a 10% reserve. Missing telemetry uses a 45s fallback.
- Natural time since the previous grant counts. The wait is only the remainder until the next sustainable time.
- This is client-side pacing of valid requests. It is not a rate-limit bypass, a cheaper request, or a billing change. Flex is not enabled.

## Test plan

- [ ] `cargo test -p codex-slow-mode`
- [ ] `cargo run -p codex-slow-mode --bin slow-mode-simulate` and confirm adaptive runs insert idle time while `quality_changes` stays 0
- [ ] In a session, `/slow-mode` then `/slow-mode status` shows ON without a model turn
- [ ] `/fast on` while Slow Mode is on prints the conflict locally
- [ ] `/slow-mode off` during a pacing wait lets the queued model request proceed
- [ ] Confirm `~/.codex/config.toml` service tier is unchanged after enable and disable
- [ ] `just write-app-server-schema` and `just write-app-server-schema --experimental`, then `just test -p codex-app-server-protocol`
- [ ] Existing `codex-core` and `codex-tui` tests for the touched packages

## Files

See the PR file list. The pacing crate is `codex-rs/slow-mode`. The request gate is `ModelClientSession::stream`. The command crosses the TUI / app-server boundary as `thread/slowMode`, which does not admit a turn.
