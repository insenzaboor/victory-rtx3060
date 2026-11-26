# Run analysis tools

Python 3 (Pillow for the screenshot sheet). They read a run folder (`runs/<tag>/`: `autopilot.log`,
`shots/`, `timeline.*`, `run.txt`) and its log (`logs/run-<tag>-*.log.err`), and never start the game.

| Tool | What it does |
|---|---|
| `run-report.py <tag> [--baseline <tag>]` | `report.html` / `report.md` / `report.json`: verdict, load time, walk stats (median ms, ms per 1k draws after the first minute), A/B phases per switch combination with a winner and a noise hint, RAM timeline, top warnings, screenshot sheet, comparison with a baseline |
| `triage.py <tag>... [--brief] [--json]` | How a run ended, by signature: known load hang, title GPU watchdog, profiler crash, RAM-monitor kill, device removed, or unknown with the symbolized faulting stack |
| `triage-signatures.json` | The signatures, tried in priority order; add one as plain JSON |
| `runlib.py` | Shared parsing (caption lines, A/B phases, memory lines, thread dumps, resource log) |

Measure frame time *after* the first minute in the world: streaming makes those frames 3-8x costlier per draw.
