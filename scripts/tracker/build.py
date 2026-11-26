"""Builds the project's tracking views from docs/data/ports.json and docs/data/runs.json.

Outputs (all committed, so GitHub renders them):
  docs/assets/progress/*.svg   stacked progress bars (overall and per category)
  docs/assets/charts/frametime.png   frame time per run
  docs/TRACKER.md               every port, by category and status
  README.md                     the block between <!-- PROGRESS:START --> and <!-- PROGRESS:END -->

Usage: python scripts/tracker/build.py   (from the repository root; needs matplotlib)
"""
import json
import os
import re

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
REPO = "https://github.com/insenzaboor/victory-rtx3060"
DATA = os.path.join(ROOT, "docs", "data")

ports = json.load(open(os.path.join(DATA, "ports.json"), encoding="utf-8"))
runs = json.load(open(os.path.join(DATA, "runs.json"), encoding="utf-8"))
items = ports["items"]

# Done = decided and in its final state; in flight = merged or being built; planned = not started.
DONE = {"shipped", "no-gain", "not-applicable"}
FLIGHT = {"default-untested", "ready", "in-progress"}
COLORS = {"done": "#2ea043", "flight": "#3b82f6", "planned": "#6e7681"}
BADGE = {
    "shipped": "🟢 shipped",
    "default-untested": "🔵 on, untested",
    "ready": "🔵 ready to test",
    "in-progress": "🟡 in progress",
    "no-gain": "⚪ no gain (off)",
    "not-applicable": "⚫ n/a",
    "planned": "⬜ planned",
}
ORDER = ["shipped", "default-untested", "ready", "in-progress", "planned", "no-gain", "not-applicable"]


def counts(group):
    d = sum(1 for i in group if i["status"] in DONE)
    f = sum(1 for i in group if i["status"] in FLIGHT)
    return d, f, len(group) - d - f, len(group)


def bar_svg(title, group, width=560):
    d, f, p, n = counts(group)
    w = width - 20
    seg = [(d, COLORS["done"]), (f, COLORS["flight"]), (p, COLORS["planned"])]
    x, rects = 10, []
    for k, color in seg:
        if k:
            ww = w * k / n
            rects.append(f'<rect x="{x:.1f}" y="30" width="{ww:.1f}" height="16" fill="{color}"/>')
            x += ww
    pct = round(100 * d / n)
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="70" role="img" aria-label="{title}: {pct}% done">
<style>text{{font-family:-apple-system,Segoe UI,Helvetica,Arial,sans-serif;fill:#768390;font-size:12px}} .t{{fill:#768390;font-size:14px;font-weight:600}}</style>
<text class="t" x="10" y="20">{title}</text><text x="{width - 10}" y="20" text-anchor="end">{pct}% done · {d + f} of {n} landed</text>
<rect x="10" y="30" width="{w}" height="16" rx="8" fill="#8c959f" fill-opacity="0.25"/>
<clipPath id="c"><rect x="10" y="30" width="{w}" height="16" rx="8"/></clipPath><g clip-path="url(#c)">{"".join(rects)}</g>
<text x="10" y="64">■ {d} done</text><text x="120" y="64" style="fill:{COLORS["flight"]}">■ {f} merged / in flight</text><text x="300" y="64" style="fill:{COLORS["planned"]}">■ {p} planned</text>
</svg>
'''


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, "w", encoding="utf-8", newline="\n").write(text)


# Progress bars.
prog = os.path.join(ROOT, "docs", "assets", "progress")
write(os.path.join(prog, "overall.svg"), bar_svg("Everything ported and evaluated", items))
for cat, label in ports["categories"].items():
    write(os.path.join(prog, f"{cat}.svg"), bar_svg(label, [i for i in items if i["category"] == cat]))

# Frame-time chart.
try:
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    rs = runs["runs"]
    xs = list(range(len(rs)))  # evenly spaced: close runs no longer overlap
    ys = [r["ms"] for r in rs]
    fig, ax = plt.subplots(figsize=(10, 4.2), dpi=110)
    fig.patch.set_facecolor("#0d1117")
    ax.set_facecolor("#0d1117")
    ax.plot(xs, ys, color="#3b82f6", lw=2.5, marker="o", ms=7, zorder=3)
    ax.fill_between(xs, ys, max(ys) * 1.05, color="#3b82f6", alpha=0.06)
    for x, r in zip(xs, rs):
        ax.annotate(f'{r["ms"]} ms', (x, r["ms"]), textcoords="offset points", xytext=(0, 9), ha="center", color="#c9d1d9", fontsize=9)
    ax.axhline(33.3, color="#2ea043", ls="--", lw=1)
    ax.text(xs[0], 38, "30 fps", color="#2ea043", fontsize=9)
    ax.set_ylim(0, max(ys) * 1.12)
    ax.set_xticks(xs, [f"run {r['run']}" for r in rs])
    ax.set_xlabel("measured runs", color="#8b949e")
    ax.set_ylabel("ms per frame (~3,000 draws)", color="#8b949e")
    ax.set_title("Frame time in a heavy scene, run by run (RTX 3060)", color="#c9d1d9", fontsize=12, loc="left")
    ax.tick_params(colors="#8b949e")
    for s in ax.spines.values():
        s.set_color("#30363d")
    ax.grid(color="#21262d", lw=0.8)
    fig.tight_layout()
    out = os.path.join(ROOT, "docs", "assets", "charts")
    os.makedirs(out, exist_ok=True)
    fig.savefig(os.path.join(out, "frametime.png"), facecolor=fig.get_facecolor())
except ImportError:
    print("matplotlib missing: chart not rebuilt")


def issue_link(i):
    n = i.get("issue")
    return f"[#{n}]({REPO}/issues/{n})" if n else ""


def commit_link(i):
    c = i.get("commit")
    return f"[`{c}`]({REPO}/commit/{c})" if c else ""


# TRACKER.md
lines = [
    "# Port tracker",
    "",
    f"_Generated from [`docs/data/ports.json`](data/ports.json) by `scripts/tracker/build.py` · updated {ports['updated']}_ · "
    f"**[Live dashboard](https://insenzaboor.github.io/victory-rtx3060/)** · **[Project board](https://github.com/users/insenzaboor/projects/2)**",
    "",
    "![overall](assets/progress/overall.svg)",
    "",
    "| Status | Meaning | Count |",
    "|---|---|---|",
]
for s in ORDER:
    lines.append(f"| {BADGE[s]} | {ports['statuses'][s]} | {sum(1 for i in items if i['status'] == s)} |")
for cat, label in ports["categories"].items():
    group = sorted([i for i in items if i["category"] == cat], key=lambda i: ORDER.index(i["status"]))
    lines += ["", f"## {label}", "", f"![{cat}](assets/progress/{cat}.svg)", "",
              "| Item | Status | Source | Switch | Effect | Commit | Issue |", "|---|---|---|---|---|---|---|"]
    for i in group:
        sw = f"`{i['switch']}`" if i.get("switch") else ""
        lines.append(f"| {i['title']} | {BADGE[i['status']]} | {i['source']} | {sw} | {i.get('effect', '')} | {commit_link(i)} | {issue_link(i)} |")
write(os.path.join(ROOT, "docs", "TRACKER.md"), "\n".join(lines) + "\n")

# README progress block.
readme = os.path.join(ROOT, "README.md")
if os.path.exists(readme):
    text = open(readme, encoding="utf-8").read()
    d, f, p, n = counts(items)
    block = ["<!-- PROGRESS:START -->", "", '<p align="center"><img src="docs/assets/progress/overall.svg" alt="overall progress"></p>', ""]
    block.append("| | Category | Done | In flight | Planned |")
    block.append("|---|---|---|---|---|")
    for cat, label in ports["categories"].items():
        cd, cf, cp, cn = counts([i for i in items if i["category"] == cat])
        block.append(f'| <img src="docs/assets/progress/{cat}.svg" width="300"> | **{label}** | {cd} | {cf} | {cp} |')
    block += ["", f"**{d} of {n}** items done or decided, **{f}** merged and waiting for a game run, **{p}** planned · "
              "full list in **[docs/TRACKER.md](docs/TRACKER.md)** · charts in the **[live dashboard](https://insenzaboor.github.io/victory-rtx3060/)**", "",
              "<!-- PROGRESS:END -->"]
    text = re.sub(r"<!-- PROGRESS:START -->.*?<!-- PROGRESS:END -->", "\n".join(block), text, flags=re.S)
    write(readme, text)
print("built:", len(items), "items,", len(runs["runs"]), "runs")
