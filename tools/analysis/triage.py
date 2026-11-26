"""Classifies how a run ended, from its logs and thread dumps, against tools/triage-signatures.json.

Usage:
  C:\\Python314\\python.exe -I tools\\triage.py <tag> [<tag> ...]   verdict, evidence and (crashes, hangs) the stacks
  C:\\Python314\\python.exe -I tools\\triage.py --brief fortyeight thirtythree   one line per run
  C:\\Python314\\python.exe -I tools\\triage.py <tag> --json                     the result as JSON on stdout
  --force-symbols  symbolizes with a rebuilt exe too (names will be wrong for that run's offsets)
  --no-write  does not write runs\\<tag>\\triage.json (written by default; run-queue.ps1 reads it)

Verdicts (first matching signature by priority): RAM-monitor kill, device removed / TDR, title GPU watchdog
crash (title+0x23cf683), profiler crash (crash.rs / stacks.rs frames, symbolized with addr2sym.py on the exe
named in runs\\<tag>\\run.txt; when that exe was rebuilt since, a heuristic fallback is used and marked so),
known load hang (title-main at title+0x56a0d < +0x225130 in the thread dump), stuck loads, good endings
(time limit, -StopAfter), else unknown: then the symbolized stacks of the faulting thread and title-main.
New signatures: add an entry to tools\\triage-signatures.json (its 'about' lists the conditions).
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import runlib  # noqa: E402


def triage(tag, force=False):
    run = runlib.Run(tag)
    matches, ctx = runlib.classify(run)
    el = run.errlog
    tm = el["title_module"]
    top = matches[0]
    res = {
        "tag": tag, "verdict": top["id"], "name": top["name"], "class": top["class"], "evidence": top["evidence"],
        "heuristic": top["heuristic"], "advice": top["advice"], "also": [{"id": m["id"], "name": m["name"]} for m in matches[1:]],
        "world": bool(run.world_reached), "load_s": run.load_seconds, "exit": run.runlog["exit"], "log": run.err,
        "exe": run.exe, "symbols_exe": ctx.exe, "symbols_reliable": ctx.reliable, "symbols_note": ctx.exe_note,
        "notes": run.notes + ([] if run.exists else [f"runs\\{tag} does not exist"]) + ([] if el["exists"] else ["no .err run log found"]),
        "stacks": {},
    }
    exc = runlib.faulting(el)
    want_fault = top["class"] in ("crash", "unknown", "gpu") or (exc and exc["kind"] == "unhandled")
    want_main = top["class"] in ("hang", "unknown")
    if exc and want_fault:
        res["stacks"]["faulting"] = {"header": exc["text"], "line": exc["line"], "frames": runlib.stack_lines(exc, ctx, tm, force=force), "title_said": exc["said"][:4]}
    if want_main:
        tmain = runlib.thread_in_dumps(el, "title-main")
        if tmain:
            d, t = tmain[-1]
            res["stacks"]["title-main"] = {"header": f"title-main (thread {t['tid']}) in the dump at line {d['line']}", "line": t["line"],
                                           "frames": runlib.stack_lines(t, ctx, tm, force=force)}
        elif el["dumps"]:
            res["stacks"]["title-main"] = {"header": "no title-main thread in the dump", "frames": []}
        else:
            res["stacks"]["title-main"] = {"header": "no thread dump in this run", "frames": []}
    return res


def show(res):
    print(f"== {res['tag']}: {res['name']}  [{res['class']}]")
    for e in res["evidence"]:
        print(f"   evidence: {e}")
    if res["heuristic"]:
        print(f"   heuristic: {res['heuristic']}")
    if res["advice"]:
        print(f"   advice: {res['advice']}")
    if res["also"]:
        print("   also matched: " + "; ".join(a["name"] for a in res["also"]))
    after = f" after {res['load_s']} s" if res["load_s"] else ""
    print(f"   world: {'yes' if res['world'] else 'no'}{after}; exit code {res['exit']}")
    for n in res["notes"]:
        print(f"   note: {n}")
    for key, st in res["stacks"].items():
        print(f"   --- {key}: {st['header']}")
        if key == "faulting" or any("victory.exe" in f for f in st["frames"]):
            print(f"       symbols: {res['symbols_exe']} ({'reliable' if res['symbols_reliable'] else 'left out, --force-symbols shows them anyway'}: {res['symbols_note']})")
        for f in st["frames"]:
            print(f"       {f}")
        for s in st.get("title_said", []):
            print(f"       title said: {s[:160]}")


def main(argv):
    args = [a for a in argv if not a.startswith("--")]
    flags = {a for a in argv if a.startswith("--")}
    if not args or "--help" in flags:
        print(__doc__)
        return 2
    out = []
    for tag in args:
        res = triage(tag, "--force-symbols" in flags)
        out.append(res)
        if "--no-write" not in flags and os.path.isdir(os.path.join(runlib.RUNS, tag)):
            with open(os.path.join(runlib.RUNS, tag, "triage.json"), "w", encoding="utf-8") as f:
                json.dump(res, f, indent=2, default=str)
        if "--json" in flags:
            continue
        if "--brief" in flags:
            load = f", world in {res['load_s']} s" if res["load_s"] else ""
            print(f"{tag:<14} {res['class']:<8} {res['name']}{load}{'  (heuristic)' if res['heuristic'] else ''}")
        else:
            show(res)
    if "--json" in flags:
        print(json.dumps(out if len(out) > 1 else out[0], indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
