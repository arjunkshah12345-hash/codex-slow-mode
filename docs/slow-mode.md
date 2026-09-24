# Codex Slow Mode

I have plenty of wall-clock time. I do not want an autonomous coding task to burn through my five-hour Codex allowance in a short burst.

## What it does

Slow Mode spaces model inference requests according to the usage and reset timing Codex already records.

After `/slow-mode`, the session keeps the same model, the same reasoning effort, the same tools, and the same checks. The only change is when the next Responses request is allowed to start. Local work such as shell commands, tests, compilation, file reads, and MCP tool execution is not delayed. Time those tools already spend counts toward the gap before the next model request.

While Slow Mode is on, model requests for the session share one gate, so follow-up turns, reviews, and subagent model calls run one at a time. Subagent calls are queued, not dropped.

The pacer reads the primary and secondary rate-limit windows (`used_percent`, window length, `resets_at`). It keeps a smoothed estimate of how much quota a request actually consumed and aims to spend the remaining allowance, minus a 10% reserve, across the time left until the primary reset. If live telemetry is missing or stale, it falls back to a 45 second gap and switches to the adaptive schedule once a trustworthy snapshot arrives. A secondary window that is nearly exhausted can lengthen the wait. Multi-day secondary math is capped at 15 minutes of actual sleep while the primary window still has usable quota, and the status text says that the longer-term allowance is what is being preserved.

## What it does not do

Slow Mode does not:

- make a single model request cheaper
- guarantee a particular OpenAI quota
- bypass or circumvent OpenAI rate limits
- manipulate billing
- reduce the model's intelligence
- intentionally degrade output quality

It only controls when legitimate model requests are submitted. It does not rotate accounts, replay credentials, spoof identity, or rewrite usage reports.

## Usage

```text
codex

> /slow-mode
Slow mode enabled for this session.

> Build the feature...

> /slow-mode status
Slow mode: ON
Model: gpt-5.4
Fast mode: OFF
Primary usage: 43%
Primary reset: 2h 51m
Estimated current request cost: 0.8%
Next eligible inference: ~1m 13s
Queued model requests: 1

> /slow-mode off
Slow mode disabled for this session.
```

Bare `/slow-mode` means on. `/slow-mode on` is the same. These commands are handled locally. They do not send a prompt to the model.

The choice is session scoped. It does not write `config.toml`. The next `codex` process starts at the normal pace unless you enable Slow Mode again.

Fast mode and Slow Mode cannot be active together. Enabling Slow Mode moves the session off the Fast service tier and restores that previous session tier when you turn Slow Mode off. It does not overwrite the saved Fast preference. `/fast on` during Slow Mode prints a local explanation and does not call the model.

The footer shows `SLOW` for the rest of the session. During a wait the status line updates in place:

```text
Slow mode · pacing usage · next model request in ~1m 42s
```

## Install

This patch is meant to land in the official `codex` binary. Until it does, build this tree and install a separate command so an existing official install is left alone:

```shell
./scripts/install-codex-slow.sh
```

That places `codex-slow` on `~/.local/bin`. Run `codex-slow`, then `/slow-mode`.

## Architecture

```text
/slow-mode in the TUI
        |
        v
thread/slowMode RPC   (no turn, no model call)
        |
        v
session registry (thread id, parent thread id, rate-limit snapshots)
        |
        v
SlowModePacer + SlowModeRateController
        |
        v
ModelClientSession::stream, immediately before the Responses request
```

The permit is held until the response stream is dropped, so one in-flight model request occupies the gate. Shell work after the stream ends is not paced. Retries call `stream` again and pass through the same gate. WebSocket prewarm does not. Ctrl-C cancels a pacing wait through the existing interrupt path and leaves Slow Mode enabled for the next turn. `/slow-mode off` ends the wait and lets the request continue. Session shutdown does not sit in the pacer.

ChatGPT subscription sessions use pacing and the standard service tier. API-key sessions use the same pacer. If the account has no `used_percent` window, the 45 second fallback applies. Slow Mode does not send `service_tier=flex`. Flex is a separate OpenAI API option, and this client does not claim that Flex reduces a ChatGPT subscriber's five-hour allowance.

## Simulation

```shell
cargo run -p codex-slow-mode --bin slow-mode-simulate
```

The simulator compares normal, fixed-delay, and adaptive pacing on the same request costs. Quality-affecting changes stay at zero. The adaptive column is the one that spreads a burst across the window instead of exhausting it before reset.

## Tests

```shell
cargo test -p codex-slow-mode
```

The suite covers slash-argument parsing, the Fast/Slow latch, fake-clock gating, trajectory and elapsed-time pacing, serialization of parallel acquires, cancellation, disable-during-wait, reset handling, stale telemetry, quantized `used_percent`, secondary-window pressure, and unchanged behavior when the pacer is off.

After the protocol change, regenerate the checked-in app-server schema before relying on those fixtures:

```shell
just write-app-server-schema
just write-app-server-schema --experimental
```

## Limits that need the server

The client can delay a request. It cannot:

- make the server charge less for the same request
- see an exact per-request quota cost (the percentage is quantized)
- coordinate two `codex` processes that do not share a thread-id family
- ask OpenAI to schedule the request itself

A reset timestamp and `used_percent` are the signals Codex already stores. If those fields are absent, Slow Mode uses the fallback interval rather than guessing a private endpoint.
