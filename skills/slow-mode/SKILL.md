---
name: slow-mode
description: Pace this Codex session only. Use when the user runs $slow-mode at the start of a session, or asks to turn slow mode off for this session. Same model, tools, and checks. Does nothing in other sessions.
---

# Slow Mode

This skill applies only to the session where the user invoked it. Other sessions stay at the normal pace. Do not write `~/.codex/config.toml` or any saved preference.

## On

When the user runs `$slow-mode` or asks to pace this session:

1. Keep the same model, the same tools, and the same checks.
2. Send one model request at a time for the rest of this session. Later requests wait. Do not drop them.
3. Do not delay shell, tests, compilation, or file reads.
4. Do not make a request cheaper, and do not try to get around the limit.
5. Tell the user: Slow mode is on for this session.

## Off

When the user says slow mode off, or runs `$slow-mode off`:

1. Stop pacing. This session returns to the normal pace.
2. Tell the user: Slow mode is off for this session.

A new session does not inherit this. The user runs `$slow-mode` again if they want it.
