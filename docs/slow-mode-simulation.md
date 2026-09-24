# Slow Mode simulation

Deterministic fake clock. Quality-affecting changes are model, reasoning, tool, and prompt changes. Slow Mode keeps that count at zero and only moves request start times.

| Workload | Mode | Completion | Peak use before reset | Exhausted before reset | Inserted idle | Requests | Quality changes |
| --- | --- | ---: | ---: | --- | ---: | ---: | ---: |
| many cheap turns | normal | 8s | 26.0% | no | 0s | 40 | 0 |
| many cheap turns | fixed-delay | 29m 15s | 26.0% | no | 29m 07s | 40 | 0 |
| many cheap turns | adaptive | 5h 14m | 18.0% | no | 5h 14m | 40 | 0 |
| few expensive turns | normal | 12s | 44.0% | no | 0s | 6 | 0 |
| few expensive turns | fixed-delay | 3m 47s | 44.0% | no | 3m 35s | 6 | 0 |
| few expensive turns | adaptive | 2h 00m | 44.0% | no | 2h 00m | 6 | 0 |
| mixed-cost turns | normal | 30s | 35.4% | no | 0s | 8 | 0 |
| mixed-cost turns | fixed-delay | 5m 15s | 35.4% | no | 4m 45s | 8 | 0 |
| mixed-cost turns | adaptive | 1h 45m | 35.4% | no | 1h 44m | 8 | 0 |
| parallel subagents | normal | 2s | 36.0% | no | 0s | 4 | 0 |
| parallel subagents | fixed-delay | 2m 15s | 36.0% | no | 2m 13s | 4 | 0 |
| parallel subagents | adaptive | 27m 16s | 36.0% | no | 27m 14s | 4 | 0 |
| long-running shell commands | normal | 32m 02s | 43.0% | no | 0s | 8 | 0 |
| long-running shell commands | fixed-delay | 32m 02s | 43.0% | no | 0s | 8 | 0 |
| long-running shell commands | adaptive | 32m 02s | 43.0% | no | 0s | 8 | 0 |
| 50% already consumed | normal | 6s | 80.0% | no | 0s | 30 | 0 |
| 50% already consumed | fixed-delay | 21m 45s | 80.0% | no | 21m 39s | 30 | 0 |
| 50% already consumed | adaptive | 3h 12m | 62.0% | no | 3h 12m | 30 | 0 |
| 80% already consumed | normal | 5s | 110.0% | yes | 0s | 25 | 0 |
| 80% already consumed | fixed-delay | 18m 00s | 110.0% | yes | 17m 55s | 25 | 0 |
| 80% already consumed | adaptive | 2h 15m | 88.4% | no | 2h 15m | 25 | 0 |
| 95% already consumed | normal | 2s | 105.0% | yes | 0s | 10 | 0 |
| 95% already consumed | fixed-delay | 6m 45s | 105.0% | yes | 6m 43s | 10 | 0 |
| 95% already consumed | adaptive | 26m 45s | 95.0% | no | 26m 43s | 10 | 0 |
| reset imminent | normal | 0s | 86.0% | no | 0s | 8 | 0 |
| reset imminent | fixed-delay | 5m 15s | 78.0% | no | 5m 14s | 8 | 0 |
| reset imminent | adaptive | 2m 32s | 86.0% | no | 2m 32s | 8 | 0 |
| missing telemetry | normal | 1s | 12.0% | no | 0s | 12 | 0 |
| missing telemetry | fixed-delay | 8m 15s | 12.0% | no | 8m 13s | 12 | 0 |
| missing telemetry | adaptive | 8m 15s | 12.0% | no | 8m 13s | 12 | 0 |
| secondary nearly exhausted | normal | 3s | 42.0% | no | 0s | 15 | 0 |
| secondary nearly exhausted | fixed-delay | 10m 30s | 42.0% | no | 10m 27s | 15 | 0 |
| secondary nearly exhausted | adaptive | 3h 30m | 42.0% | no | 3h 29m | 15 | 0 |

Fallback interval when telemetry is missing: 45s. That is long enough that a burst of follow-up turns no longer fits in a couple of seconds, and short enough that the first live rate-limit sample can take over.
