"""One page per measured run: runs\\<tag>\\report.html (self-contained but for the screenshot sheet it links),
runs\\<tag>\\report.md (short) and runs\\<tag>\\report.json (the numbers, for run-queue.ps1).

Usage:
  C:\\Python314\\python.exe -I tools\\run-report.py <tag> [--baseline <tag>] [--after 60] [--no-sheet]
  C:\\Python314\\python.exe -I tools\\run-report.py fiftytwo --baseline fortynine

Contents: the verdict (world? load time, how it ended and the evidence line, from tools\\triage-signatures.json),
walk caption stats (median ms, ms per 1k draws after the first --after s of the walk, p10/p90, best and worst,
a plot of ms and draws over time), A/B phases per switch combination (frames-weighted mean ms, ms per 1k draws,
winner and a significance hint), the RAM timeline from logs\\resources.log for the run's window, the 15 most
frequent WARN/ERROR messages (digits normalised), a screenshot contact sheet (shots\\sheet.jpg) and, with
--baseline, the same metrics side by side. Missing files leave their section empty, never an error.
"""
import html
import json
import os
import sys
from datetime import datetime, timedelta

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import runlib  # noqa: E402

E = html.escape
N = runlib.num


# --- the numbers -------------------------------------------------------------------------------

def metrics(run, after):
    ap, el = run.autopilot, run.errlog
    matches, ctx = runlib.classify(run)
    top = matches[0]
    cs = runlib.caption_stats(ap, after)
    ab = runlib.ab_aggregate(el["ab"], since=ap["world_at"] + timedelta(seconds=after) if ap["world_at"] else None)
    rs = runlib.resource_summary(run.resources)
    mem = el["memory"]
    fw = (run.summary or {}).get("frames_world") or {}
    ending = {"ok": None, "killed": "killed by the RAM monitor", "hang": "stuck on the load", "crash": "crash", "gpu": "device removed",
              "unknown": "unknown"}[top["class"]]
    if top["class"] == "ok":
        ending = "time limit" if top["id"] == "time_limit" else "stopped by the autopilot"
        if run.runlog["ended_after"]:
            ending += f" ({run.runlog['ended_after']} s)"
    elif top["class"] in ("crash", "unknown", "hang"):
        ending += f": {top['name']}"
    return {
        "tag": run.tag, "exists": run.exists, "notes": run.notes, "world": bool(run.world_reached), "load_s": run.load_seconds,
        "verdict": {"id": top["id"], "name": top["name"], "class": top["class"], "ending": ending, "evidence": top["evidence"],
                    "heuristic": top["heuristic"], "also": [m["name"] for m in matches[1:]]},
        "exe": run.exe, "log": run.err, "exit": run.runlog["exit"], "started": run.started_local.isoformat(" ", "seconds") if run.started_local else None,
        "captions": cs, "ab": ab, "ram": rs,
        "layer_memory": {"peak_private_mb": max(m["private"] for m in mem), "peak_in_memory_mb": max(m["working"] for m in mem),
                         "min_pc_free_mb": min(m["free"] for m in mem), "min_commit_left_mb": min(m["left"] for m in mem)} if mem else None,
        "frames_world": {k: fw.get(k) for k in ("frames", "mean_ms", "p50_ms", "p90_ms", "p99_ms")} if fw else None,
        "warnings": [{"count": c, "what": k, "example": el["warn_example"].get(k, "")} for k, c in sorted(el["warn"].items(), key=lambda kv: -kv[1])[:15]],
        "warn_total": el["warn_total"], "warn_kinds": len(el["warn"]), "errors": el["errors"], "stalls": el["stalls"],
        "title_count": el["title_count"], "title_last": el["title_last"][-6:],
        "settings": {k: v for k, v in run.runlog["env"].items()},
        "_ap": ap, "_res": run.resources,
    }


def flat(m):
    """The comparable numbers of one run (rows of the baseline table)."""
    late = (m["captions"] or {}).get("late") or {}
    allw = (m["captions"] or {}).get("all") or {}
    fit = (m["captions"] or {}).get("fit") or {}
    ab = m["ab"]["winner"] if m["ab"] and m["ab"]["winner"] else None
    fw = m["frames_world"] or {}
    lm = m["layer_memory"] or {}
    return [
        ("Reached the world", "yes" if m["world"] else "no", None),
        ("Load time (s)", m["load_s"], "low"),
        ("Ended", m["verdict"]["ending"], None),
        (f"Walk median ms (after {m['captions']['after_s']} s)", late.get("median_ms"), "low"),
        ("Walk ms per 1k draws", late.get("ms_per_1k"), "low"),
        ("Walk p10 ms", late.get("p10_ms"), "low"),
        ("Walk p90 ms", late.get("p90_ms"), "low"),
        ("Walk median draws", late.get("median_draws"), None),
        ("Best caption ms", late.get("best", (None,))[0] if late else None, "low"),
        ("Worst caption ms", late.get("worst", (None,))[0] if late else None, "low"),
        ("Whole walk median ms", allw.get("median_ms"), "low"),
        ("Fit: ms at 3k draws", fit.get("at_3k_ms"), "low"),
        ("Frames in the world: p50 ms", fw.get("p50_ms"), "low"),
        ("Frames in the world: p90 ms", fw.get("p90_ms"), "low"),
        ("A/B best (ms per 1k draws)", ab["ms_per_1k"] if ab else None, "low"),
        ("Min free RAM (GB)", m["ram"]["min_free_gb"], "high"),
        ("Max commit (GB)", m["ram"]["max_commit_gb"], "low"),
        ("Max page-in (MB/s)", m["ram"]["max_pagein_mbps"], "low"),
        ("Max victory private (GB)", m["ram"]["max_private_gb"], "low"),
        ("Layer peak private (MB)", lm.get("peak_private_mb"), "low"),
        ("Warnings (total)", m["warn_total"], "low"),
        ("Warning kinds", m["warn_kinds"], "low"),
    ]


# --- pictures --------------------------------------------------------------------------------

def contact_sheet(run_dir, cols=6, width=300):
    """shots\\*.jpg -> shots\\sheet.jpg, labelled with each file's name; None without shots or PIL."""
    shots = os.path.join(run_dir, "shots")
    files = sorted(f for f in os.listdir(shots) if f.lower().endswith((".jpg", ".png")) and f != "sheet.jpg") if os.path.isdir(shots) else []
    if not files:
        return None, 0
    try:
        from PIL import Image, ImageDraw
    except ImportError:
        return None, len(files)
    out = os.path.join(shots, "sheet.jpg")
    try:
        if os.path.getmtime(out) >= max(os.path.getmtime(os.path.join(shots, f)) for f in files):
            return out, len(files)
    except OSError:
        pass
    thumbs = []
    for f in files:
        try:
            im = Image.open(os.path.join(shots, f))
            im.draft("RGB", (width * 2, width * 2))   # JPEG decodes at a fraction of the size: cheap
            im = im.convert("RGB")
            im.thumbnail((width, width * 9 // 16 + 40))
            thumbs.append((f, im))
        except OSError:
            continue
    if not thumbs:
        return None, len(files)
    th = max(im.size[1] for _, im in thumbs)
    cell_h = th + 18
    rows = (len(thumbs) + cols - 1) // cols
    sheet = Image.new("RGB", (cols * (width + 6) + 6, rows * (cell_h + 6) + 6), (24, 24, 27))
    d = ImageDraw.Draw(sheet)
    for i, (f, im) in enumerate(thumbs):
        x = 6 + (i % cols) * (width + 6)
        y = 6 + (i // cols) * (cell_h + 6)
        sheet.paste(im, (x, y))
        d.text((x + 2, y + th + 3), os.path.splitext(f)[0], fill=(220, 220, 225))
    sheet.save(out, quality=82)
    return out, len(thumbs)


def svg_lines(series, x0, markers=(), width=880, height=230, left_label="", right_label="", left_max=None, right_max=None):
    """series: [(name, css class, axis 'l'/'r', [(datetime, value)])]. A small two-axis line plot."""
    pts = [p for s in series for p in s[3]]
    if len(pts) < 2:
        return '<p class="muted">Not enough samples to plot.</p>'
    ml, mr, mt, mb = 52, 56, 12, 28
    w, h = width - ml - mr, height - mt - mb
    xs = [(p[0] - x0).total_seconds() for p in pts] + [(m[0] - x0).total_seconds() for m in markers]
    xmin, xmax = min(0.0, min(xs)), max(xs)
    xmax = xmax if xmax > xmin else xmin + 1

    def top(axis, given):
        v = [p[1] for s in series if s[2] == axis for p in s[3] if p[1] is not None]
        if given:
            return given
        if not v:
            return 1
        hi = runlib.pct(v, 0.98) * 1.15 if len(v) > 10 else max(v) * 1.1
        return max(hi, 1e-6)

    ly, ry = top("l", left_max), top("r", right_max)
    X = lambda t: ml + w * ((t - x0).total_seconds() - xmin) / (xmax - xmin)  # noqa: E731
    Y = lambda v, axis: mt + h - h * min(v, ly if axis == "l" else ry) / (ly if axis == "l" else ry)  # noqa: E731
    o = [f'<svg viewBox="0 0 {width} {height}" class="plot" role="img" aria-label="{E(left_label)} and {E(right_label)} over time">']
    for i in range(5):
        y = mt + h * i / 4
        o.append(f'<line x1="{ml}" y1="{y:.1f}" x2="{ml + w}" y2="{y:.1f}" class="grid"/>')
        o.append(f'<text x="{ml - 6}" y="{y + 4:.1f}" class="tick" text-anchor="end">{ly * (1 - i / 4):.0f}</text>')
        if any(s[2] == "r" for s in series):
            o.append(f'<text x="{ml + w + 6}" y="{y + 4:.1f}" class="tick">{ry * (1 - i / 4):.0f}</text>')
    step = 60 if xmax - xmin <= 900 else 120
    t = 0
    while t <= xmax:
        x = ml + w * (t - xmin) / (xmax - xmin)
        o.append(f'<text x="{x:.1f}" y="{height - 8}" class="tick" text-anchor="middle">{int(t)} s</text>')
        t += step
    for k, (when, label) in enumerate(markers):
        x = X(when)
        ly_ = mt + 10 + 12 * (k % 2)   # alternate rows so close markers' labels do not overlap
        o.append(f'<line x1="{x:.1f}" y1="{mt}" x2="{x:.1f}" y2="{mt + h}" class="marker"/><text x="{x + 3:.1f}" y="{ly_}" class="mlabel">{E(label)}</text>')
    for name, cls, axis, data in series:
        if len(data) < 2:
            continue
        p = " ".join(f"{X(t):.1f},{Y(v, axis):.1f}" for t, v in data if v is not None)
        o.append(f'<polyline points="{p}" class="{cls}" fill="none"/>')
    o.append(f'<text x="8" y="{mt + h / 2:.0f}" class="axis" transform="rotate(-90 12 {mt + h / 2:.0f})" text-anchor="middle">{E(left_label)}</text>')
    if right_label:
        o.append(f'<text x="{width - 8}" y="{mt + h / 2:.0f}" class="axis" transform="rotate(90 {width - 12} {mt + h / 2:.0f})" text-anchor="middle">{E(right_label)}</text>')
    o.append("</svg>")
    legend = "".join(f'<span class="key {cls}">{E(name)}</span>' for name, cls, _, _ in series)
    return f'<div class="legend">{legend}</div>' + "".join(o)


# --- the page --------------------------------------------------------------------------------

CSS = """
:root{--bg:#fbfbfa;--panel:#ffffff;--ink:#1d1d1f;--muted:#6b6b70;--line:#e3e3e0;--accent:#2563eb;--accent2:#d97706;--good:#15803d;--bad:#b91c1c;--warn:#a16207;--code:#f3f3f1}
@media (prefers-color-scheme: dark){:root:not([data-theme="light"]){--bg:#121214;--panel:#1b1b1e;--ink:#ececee;--muted:#9b9ba3;--line:#2c2c31;--accent:#60a5fa;--accent2:#fbbf24;--good:#4ade80;--bad:#f87171;--warn:#facc15;--code:#232328}}
:root[data-theme="dark"]{--bg:#121214;--panel:#1b1b1e;--ink:#ececee;--muted:#9b9ba3;--line:#2c2c31;--accent:#60a5fa;--accent2:#fbbf24;--good:#4ade80;--bad:#f87171;--warn:#facc15;--code:#232328}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--ink);font:14px/1.5 "Segoe UI",system-ui,-apple-system,sans-serif}
main{max-width:1000px;margin:0 auto;padding:20px 16px 60px}h1{font-size:22px;margin:0 0 4px}h2{font-size:16px;margin:28px 0 8px;padding-top:8px;border-top:1px solid var(--line)}
.muted{color:var(--muted)}.verdict{background:var(--panel);border:1px solid var(--line);border-left:5px solid var(--muted);border-radius:8px;padding:12px 14px;margin:12px 0}
.verdict.ok{border-left-color:var(--good)}.verdict.bad{border-left-color:var(--bad)}.verdict.warn{border-left-color:var(--warn)}.verdict b{font-size:15px}
code,pre{font:12px/1.45 Consolas,"Cascadia Mono",monospace;background:var(--code);border-radius:4px}code{padding:1px 4px}pre{padding:8px 10px;overflow-x:auto;white-space:pre-wrap;word-break:break-word}
table{border-collapse:collapse;width:100%;background:var(--panel);border:1px solid var(--line);border-radius:6px;overflow:hidden;font-variant-numeric:tabular-nums}
th,td{padding:5px 8px;border-bottom:1px solid var(--line);text-align:left;vertical-align:top}th{color:var(--muted);font-weight:600;font-size:12px}td.n{text-align:right;white-space:nowrap}
.tw{overflow-x:auto}.better{color:var(--good)}.worse{color:var(--bad)}.stats{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:8px;margin:8px 0}
.stat{background:var(--panel);border:1px solid var(--line);border-radius:6px;padding:8px 10px}.stat .v{font-size:19px;font-weight:600}.stat .k{color:var(--muted);font-size:12px}
svg.plot{width:100%;height:auto;background:var(--panel);border:1px solid var(--line);border-radius:6px}svg .grid{stroke:var(--line)}svg .tick,svg .axis,svg .mlabel{fill:var(--muted);font-size:11px}
svg .marker{stroke:var(--muted);stroke-dasharray:3 3}svg .s1{stroke:var(--accent);stroke-width:1.8}svg .s2{stroke:var(--accent2);stroke-width:1.4;opacity:.85}
.legend{margin:4px 0;font-size:12px;color:var(--muted)}.key{margin-right:14px}.key:before{content:"";display:inline-block;width:14px;height:3px;margin-right:5px;vertical-align:middle;background:var(--muted)}
.key.s1:before{background:var(--accent)}.key.s2:before{background:var(--accent2)}img.sheet{width:100%;border-radius:6px;border:1px solid var(--line)}
details summary{cursor:pointer;color:var(--muted)}
"""


def stat(k, v):
    return f'<div class="stat"><div class="v">{E(str(v))}</div><div class="k">{E(k)}</div></div>'


def table(head, rows, num_cols=()):
    o = ['<div class="tw"><table><tr>' + "".join(f"<th>{E(h)}</th>" for h in head) + "</tr>"]
    for r in rows:
        o.append("<tr>" + "".join(f'<td class="n">{c}</td>' if i in num_cols else f"<td>{c}</td>" for i, c in enumerate(r)) + "</tr>")
    o.append("</table></div>")
    return "".join(o)


def verdict_html(m):
    v = m["verdict"]
    cls = "ok" if v["class"] == "ok" else ("warn" if v["class"] == "killed" else "bad")
    world = f"Reached the world in {N(m['load_s'], 0)} s" if m["world"] else "Did not reach the world"
    ev = "".join(f"<div><code>{E(e)}</code></div>" for e in v["evidence"][:4])
    extra = f'<div class="muted">Heuristic: {E(v["heuristic"])}</div>' if v["heuristic"] else ""
    also = f'<div class="muted">Also matched: {E("; ".join(v["also"]))}</div>' if v["also"] else ""
    return f'<div class="verdict {cls}"><b>{E(world)}. Ended: {E(v["ending"])}.</b>{ev}{extra}{also}</div>'


def caption_section(m):
    cs = m["captions"]
    late, allw, fit = cs["late"], cs["all"], cs["fit"]
    if not allw:
        return '<p class="muted">No walk captions (no autopilot log, or the world was never reached).</p>'
    o = []
    if late:
        o.append('<div class="stats">' + stat(f"median ms after {cs['after_s']} s", N(late["median_ms"])) + stat("ms per 1k draws", N(late["ms_per_1k"])) +
                 stat("p10 / p90 ms", f"{N(late['p10_ms'])} / {N(late['p90_ms'])}") + stat("median draws", N(late["median_draws"], 0)) +
                 stat("best ms (draws)", f"{N(late['best'][0])} ({N(late['best'][1], 0)})") + stat("worst ms (draws)", f"{N(late['worst'][0])} ({N(late['worst'][1], 0)})") +
                 "</div>")
        o.append(f'<p class="muted">{late["n"]} captions in state {"/".join(cs["states"])} from {cs["after_s"]} s into the walk; the whole walk: '
                 f'{allw["n"]} captions, median {N(allw["median_ms"])} ms, {N(allw["ms_per_1k"])} ms per 1k draws.'
                 + (f' Fit over walk and world captions (draw-fit.ps1): {N(fit["fixed_ms"])} ms + {N(fit["per_1k_ms"])} ms per 1k draws, {N(fit["at_3k_ms"])} ms at 3k draws.' if fit else "")
                 + "</p>")
    else:
        o.append(f'<p class="muted">The walk was shorter than {cs["after_s"]} s: whole walk {allw["n"]} captions, median {N(allw["median_ms"])} ms, '
                 f'{N(allw["ms_per_1k"])} ms per 1k draws.</p>')
    ap = m["_ap"]
    caps = ap["captions"]
    x0 = ap["first"] or (caps[0]["t"] if caps else None)
    if caps and x0:
        mk = []
        if ap["world_at"]:
            mk.append((ap["world_at"], "world"))
        if ap["walk_start"]:
            mk.append((ap["walk_start"], "walk"))
            mk.append((ap["walk_start"] + timedelta(seconds=cs["after_s"]), f"+{cs['after_s']} s"))
        if ap["walk_done"]:
            mk.append((ap["walk_done"], "walk done"))
        o.append(svg_lines([("ms per frame (caption)", "s1", "l", [(c["t"], c["ms"]) for c in caps]),
                            ("draws per frame", "s2", "r", [(c["t"], c["draws"]) for c in caps])], x0, mk, left_label="ms", right_label="draws"))
    return "".join(o)


def ab_section(m):
    ab = m["ab"]
    if not ab["phases"]:
        return '<p class="muted">No A/B in this run.</p>'
    if not ab["used"]:
        return f'<p class="muted">{ab["phases"]} A/B phases, none in the world (fewer than {runlib.WORLD_DRAWS} draws a frame).</p>'
    rows = []
    for c in ab["combos"]:
        sd = f'±{c["sd"]:.1f}' if c["sd"] is not None else ""
        win = " (winner)" if c is ab["winner"] else ""
        rows.append([E(c["label"]) + win, c["phases"], c["frames"], N(c["mean_ms"]), N(c["draws"], 0), f'{N(c["ms_per_1k"])} {sd}',
                     f'{c["diff"]:+.1f}' if c["diff"] is not None else "", E(c["hint"])])
    o = [f'<p><b>{E(ab["note"])}</b></p>',
         table(["Switches on", "Phases", "Frames", "Mean ms (frames-weighted)", "Draws", "ms per 1k draws (± phase spread)", "vs all off", "Hint"], rows, (1, 2, 3, 4, 5, 6))]
    if len(ab["switches"]) > 1:
        rows = [[f"<code>{E(s['switch'])}</code>", f'{N(s["on"]["ms_per_1k"])} ({s["on"]["phases"]})', f'{N(s["off"]["ms_per_1k"])} ({s["off"]["phases"]})',
                 f'{s["diff"]:+.1f}', E(s["hint"])] for s in ab["switches"]]
        o.append("<p class=muted>Each switch on its own (all phases with it on vs off):</p>")
        o.append(table(["Switch", "On: ms/1k (phases)", "Off: ms/1k (phases)", "Difference", "Hint"], rows, (1, 2, 3)))
    o.append(f'<p class="muted">{ab["used"]} of {ab["phases"]} phases used ({ab["skipped"] - ab["early"]} in menus or loading and {ab["early"]} starting in the first {m["captions"]["after_s"]} s in the world left out). The hint divides the difference '
             f'by the standard error from the spread of ms per 1k draws between phases: 2x or more is likely real.</p>')
    return "".join(o)


def ram_section(m):
    r, res = m["ram"], m["_res"]
    if not res["exists"]:
        return '<p class="muted">No logs\\resources.log.</p>'
    if not r["samples"]:
        return '<p class="muted">No resources.log samples in the run\'s time window (the monitor was not running?).</p>'
    o = ['<div class="stats">' + stat("min free RAM (GB)", N(r["min_free_gb"], 2)) + stat("max commit (GB)", N(r["max_commit_gb"], 1)) +
         stat("min free commit (GB)", N(r["min_commit_free_gb"], 1)) + stat("max page-in (MB/s)", N(r["max_pagein_mbps"])) +
         stat("page-in spikes > 80 MB/s", r["pagein_spikes"]) + stat("max victory private (GB)", N(r["max_private_gb"], 2)) + "</div>"]
    if r["top_spikes"]:
        o.append('<p class="muted">Biggest page-in samples: ' + ", ".join(f"{t} {N(p)} MB/s ({N(f, 2)} GB free)" for t, p, f in r["top_spikes"]) + ".</p>")
    if r["killed"] or r["critical"]:
        o.append("<pre>" + E("\n".join((r["critical"][-3:] + r["killed"])[-5:])) + "</pre>")
    sm = res["samples"]
    x0 = res["window"][0]
    o.append(svg_lines([("free RAM (GB)", "s1", "l", [(s["t"], s["free"]) for s in sm if s["free"] is not None]),
                        ("victory private (GB)", "s2", "l", [(s["t"], s["private"]) for s in sm if s["private"] is not None])],
                       x0, left_label="GB", left_max=max([32.0] + [s["private"] or 0 for s in sm])))
    lm = m["layer_memory"]
    if lm:
        o.append(f'<p class="muted">The layer\'s own memory lines: peak {lm["peak_private_mb"]} MB private ({lm["peak_in_memory_mb"]} MB in memory), '
                 f'PC down to {lm["min_pc_free_mb"]} MB free and {lm["min_commit_left_mb"]} MB left to commit.</p>')
    return "".join(o)


def warn_section(m):
    if not m["warnings"]:
        return '<p class="muted">No WARN or ERROR lines.</p>'
    rows = [[w["count"], f'<code>{E(w["what"])}</code><details><summary>example</summary><code>{E(w["example"])}</code></details>'] for w in m["warnings"]]
    return (f'<p class="muted">{m["warn_total"]} lines of {m["warn_kinds"]} kinds ({m["errors"]} ERROR); '
            f'{m["stalls"]} "submission line stands still" warnings.</p>' + table(["Count", "Message (digits as N)"], rows, (0,)))


def compare_section(m, b):
    if not b:
        return ""
    if not b["exists"]:
        return f'<h2>Against {E(b["tag"])}</h2><p class="muted">runs\\{E(b["tag"])} does not exist.</p>'
    rows = []
    for (k, v, better), (_, bv, _) in zip(flat(m), flat(b)):
        d = ""
        if isinstance(v, (int, float)) and isinstance(bv, (int, float)) and not isinstance(v, bool):
            diff = v - bv
            rel = f" ({100 * diff / bv:+.0f}%)" if bv else ""
            good = better and ((diff < 0) if better == "low" else (diff > 0))
            bad = better and ((diff > 0) if better == "low" else (diff < 0))
            cls = "better" if good and abs(diff) > 1e-9 else ("worse" if bad and abs(diff) > 1e-9 else "")
            d = f'<span class="{cls}">{diff:+.1f}{rel}</span>'
        rows.append([E(k), E(N(bv)), E(N(v)), d])
    diffs = []
    sa, sb = m["settings"], b["settings"]
    skip = ("VICTORY_TIMELINE", "VICTORY_PROFILE", "VICTORY_RECORD", "VICTORY_CAPTURE")
    for k in sorted(set(sa) | set(sb)):
        if k.startswith(skip):
            continue
        if sa.get(k) != sb.get(k):
            diffs.append(f"{k}: {sb.get(k, '(unset)')} -> {sa.get(k, '(unset)')}")
    exe = f"<p class=muted>exe: {E(str(b['exe']))} -> {E(str(m['exe']))}</p>" if b["exe"] != m["exe"] else ""
    return (f'<h2>Against {E(b["tag"])}</h2>' + table(["Metric", b["tag"], m["tag"], "Difference"], rows, (1, 2, 3)) + exe +
            (f'<details><summary>{len(diffs)} settings differ</summary><pre>{E(chr(10).join(diffs))}</pre></details>' if diffs else '<p class="muted">Same VICTORY_* settings.</p>'))


def page(m, b, sheet, nshots):
    title = f"Run {m['tag']}"
    v = m["verdict"]
    o = [f'<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">'
         f"<title>{E(title)}</title><style>{CSS}</style></head><body><main>",
         f"<h1>{E(title)}</h1><div class=muted>started {E(str(m['started']))} · exe <code>{E(str(m['exe']))}</code> · exit code {E(str(m['exit']))} · "
         f"report {datetime.now():%Y-%m-%d %H:%M}</div>", verdict_html(m)]
    for n in m["notes"]:
        o.append(f'<p class="muted">Note: {E(n)}</p>')
    o.append("<h2>Walk captions</h2>" + caption_section(m))
    o.append("<h2>A/B phases</h2>" + ab_section(m))
    o.append("<h2>RAM</h2>" + ram_section(m))
    o.append("<h2>Top warnings and errors</h2>" + warn_section(m))
    o.append(compare_section(m, b))
    o.append("<h2>Screenshots</h2>")
    if sheet:
        o.append(f'<p class=muted>{nshots} screenshots (file names: time and autopilot state).</p><a href="shots/sheet.jpg"><img class="sheet" src="shots/sheet.jpg" alt="contact sheet of {nshots} screenshots"></a>')
    else:
        o.append(f'<p class="muted">{"No screenshots." if not nshots else "Contact sheet not made (PIL missing or --no-sheet)."}</p>')
    if m["title_last"]:
        o.append(f'<h2>The title\'s last prints</h2><p class=muted>{m["title_count"]} lines from the title in all.</p><pre>{E(chr(10).join(m["title_last"]))}</pre>')
    o.append(f'<p class="muted">Log: <code>{E(str(m["log"]))}</code> · verdict signature <code>{E(v["id"])}</code> (tools/triage-signatures.json)</p>')
    o.append("</main></body></html>")
    return "\n".join(o)


def markdown(m, b):
    v = m["verdict"]
    late = m["captions"]["late"]
    lines = [f"# Run {m['tag']}", "",
             f"**{'Reached the world in ' + N(m['load_s'], 0) + ' s' if m['world'] else 'No world'}; ended: {v['ending']}.**",
             f"Evidence: `{v['evidence'][0] if v['evidence'] else '-'}`" + (" (heuristic)" if v["heuristic"] else ""), ""]
    if late:
        lines.append(f"- Walk after {m['captions']['after_s']} s: median {N(late['median_ms'])} ms, {N(late['ms_per_1k'])} ms per 1k draws, "
                     f"p10/p90 {N(late['p10_ms'])}/{N(late['p90_ms'])} ms, median {N(late['median_draws'], 0)} draws "
                     f"(best {N(late['best'][0])} ms at {N(late['best'][1], 0)}, worst {N(late['worst'][0])} at {N(late['worst'][1], 0)})")
    if m["ab"]["used"]:
        lines.append(f"- A/B: {m['ab']['note']}")
    r = m["ram"]
    if r["samples"]:
        lines.append(f"- RAM: min free {N(r['min_free_gb'], 2)} GB, max commit {N(r['max_commit_gb'])} GB, max page-in {N(r['max_pagein_mbps'])} MB/s "
                     f"({r['pagein_spikes']} spikes), victory private up to {N(r['max_private_gb'], 2)} GB" + ("; **KILLED**" if r["killed"] else ""))
    for w in m["warnings"][:3]:
        lines.append(f"- {w['count']}x `{w['what'][:140]}`")
    if b and b["exists"]:
        bl = (b["captions"]["late"] or {})
        if late and bl:
            lines.append(f"- vs {b['tag']}: walk median {N(bl.get('median_ms'))} -> {N(late['median_ms'])} ms, "
                         f"ms per 1k draws {N(bl.get('ms_per_1k'))} -> {N(late['ms_per_1k'])}, load {N(b['load_s'], 0)} -> {N(m['load_s'], 0)} s")
    lines += ["", "Full report: [report.html](report.html)", ""]
    return "\n".join(lines)


def main(argv):
    args, base, after, sheet_on = [], None, 60, True
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--baseline" and i + 1 < len(argv):
            base = argv[i + 1]
            i += 1
        elif a == "--after" and i + 1 < len(argv):
            after = int(argv[i + 1])
            i += 1
        elif a == "--no-sheet":
            sheet_on = False
        elif a in ("-h", "--help"):
            print(__doc__)
            return 2
        else:
            args.append(a)
        i += 1
    if len(args) != 1:
        print(__doc__)
        return 2
    run = runlib.Run(args[0])
    if not run.exists:
        print(f"runs\\{args[0]} does not exist")
        return 1
    m = metrics(run, after)
    b = metrics(runlib.Run(base), after) if base and base != args[0] else None
    sheet, nshots = contact_sheet(run.dir) if sheet_on else (None, 0)
    with open(os.path.join(run.dir, "report.html"), "w", encoding="utf-8") as f:
        f.write(page(m, b, sheet, nshots))
    with open(os.path.join(run.dir, "report.md"), "w", encoding="utf-8") as f:
        f.write(markdown(m, b))
    clean = {k: v for k, v in m.items() if not k.startswith("_")}
    clean["baseline"] = base
    with open(os.path.join(run.dir, "report.json"), "w", encoding="utf-8") as f:
        json.dump(clean, f, indent=2, default=str)
    late = m["captions"]["late"]
    v = m["verdict"]
    print(f"{run.tag}: {'world in ' + N(m['load_s'], 0) + ' s' if m['world'] else 'no world'}; ended: {v['ending']}"
          + (f"; walk median {N(late['median_ms'])} ms, {N(late['ms_per_1k'])} ms/1k draws" if late else "")
          + (f"; A/B: {m['ab']['note']}" if m["ab"]["used"] else ""))
    print(f"  evidence: {v['evidence'][0] if v['evidence'] else '-'}" + ("  (heuristic)" if v["heuristic"] else ""))
    print(f"  -> {os.path.join(run.dir, 'report.html')}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
