# Slow Mode

A Codex skill. Same model, same tools, same checks. One model request at a time, for the session where you run it.

Installing it does not replace Codex and does not write your saved config.

## Install

```shell
npx skills add arjunkshah12345-hash/codex-slow-mode --skill slow-mode -a codex -g -y
```

That is the whole install. Codex stays the same until you run the skill.

## Use

At the start of a session:

```text
$slow-mode
```

Slow mode stays on for that session. `$slow-mode off` turns it off. The next session starts at the normal pace unless you run the skill again.

## What it does

- Sends one model request at a time. Later requests wait. They are not dropped.
- Leaves shell, tests, compilation, and file reads running.
- Lasts for this session only.

## What it does not do

- Make a request cheaper
- Get around the limit
- Change quality
- Write `~/.codex/config.toml`
