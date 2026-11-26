"""Shared parsing for run-report.py and triage.py (not a command of its own).

Reads one measured run: runs\\<tag>\\ (run.txt, autopilot.log, summary.json, shots\\), the run log
logs\\run-<tag>-<stamp>.log (+ .err, .out) and logs\\resources.log for the run's time window, and
classifies how it ended against tools\\triage-signatures.json. Every reader tolerates missing files.
"""
import glob
import json
import os
import re
import statistics
import subprocess
import sys
from datetime import datetime, timedelta, timezone

TOOLS = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(TOOLS)
RUNS = os.path.join(ROOT, "runs")
LOGS = os.path.join(ROOT, "logs")
RESOURCES = os.path.join(LOGS, "resources.log")
SIGNATURES = os.path.join(TOOLS, "triage-signatures.json")
SYMBOL_ARCHIVE = os.path.join(ROOT, "build", "symbols")
WORLD_DRAWS = 500

RX_ANSI = re.compile(r"\x1b\[[0-9;]*m")
RX_LOG = re.compile(r"^(?P<ts>\d{4}-\d\d-\d\dT[\d:.]+Z)\s+(?P<lvl>TRACE|DEBUG|INFO|WARN|ERROR)\s+(?P<target>[A-Za-z0-9_:]+?):\s(?P<msg>.*)$")
RX_AB = re.compile(r"^A/B phase (?P<n>\d+) bits (?P<bits>\d+) on \[(?P<on>[^\]]*)\]: (?P<frames>\d+) frames, mean (?P<mean>[\d.]+) ms, "
                   r"median (?P<median>[\d.]+), p90 (?P<p90>[\d.]+), (?P<draws>[\d.]+) draws a frame")
RX_MEM = re.compile(r"^memory: (?P<private>\d+) MB private \((?P<working>\d+) MB of it in memory; the PC has (?P<free>\d+) MB free and (?P<left>\d+) MB left to commit\)")
RX_EXC = re.compile(r"^VIctory: (?P<kind>first-chance|unhandled) exception (?P<code>0x[0-9a-fA-F]+) at (?P<at>\S+)\s*(?:\((?P<desc>.*)\))?")
RX_DUMP = re.compile(r"^VIctory: stacks of (\d+) threads")
RX_THREAD = re.compile(r'^\s+thread (?P<tid>\d+) "(?P<name>[^"]*)" start (?P<start>\S+)')
RX_FRAME = re.compile(r"^\s+#(?P<n>\d+)\s+(?P<loc>\S+)")
RX_TITLE_SAID = re.compile(r"^\s+title said: (.*)")
RX_IMAGE = re.compile(r"image mapped base=\S+ preferred=0x140000000 path=.*?(victory-image-[0-9a-f]+\.dll)")
RX_REMOVED = re.compile(r"device removed|DXGI_ERROR_DEVICE_REMOVED|0x887a0005|DRED device state|removal reason|GetDeviceRemovedReason", re.I)
RX_CAPTION = re.compile(r"^(?P<t>\d\d:\d\d:\d\d(?:\.\d+)?) caption: (?P<fps>[\d.]+) fps (?P<ms>[\d.]+) ms (?P<draws>[\d.]+) draws \((?P<state>\w+)\)")
RX_AP = re.compile(r"^(?P<t>\d\d:\d\d:\d\d(?:\.\d+)?) (?P<what>.*)$")
RX_RES = re.compile(r"^(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d) \| (.*)$")


# --- small helpers ---------------------------------------------------------------------------

def open_text(path):
    """Opens a text file by its BOM: resources.log is UTF-16 (Windows PowerShell's Tee-Object / Out-File),
    the run logs UTF-8."""
    with open(path, "rb") as f:
        head = f.read(4)
    enc = "utf-16" if head[:2] in (b"\xff\xfe", b"\xfe\xff") else "utf-8-sig"
    return open(path, "r", encoding=enc, errors="replace")


def read_lines(path, limit=None):
    """Lines of a text file (shared read, any encoding), [] when missing."""
    if not path or not os.path.isfile(path):
        return []
    try:
        with open_text(path) as f:
            if limit:
                out = []
                for i, line in enumerate(f):
                    if i >= limit:
                        break
                    out.append(line.rstrip("\r\n"))
                return out
            return [line.rstrip("\r\n") for line in f]
    except OSError:
        return []


def parse_iso(s):
    """ISO time ('...Z', '+-hh:mm', 7 fraction digits) -> aware datetime, or None."""
    if not s:
        return None
    s = s.strip()
    s = re.sub(r"(\.\d{6})\d+", r"\1", s)
    if s.endswith("Z"):
        s = s[:-1] + "+00:00"
    try:
        d = datetime.fromisoformat(s)
    except ValueError:
        return None
    return d if d.tzinfo else d.replace(tzinfo=timezone.utc)


def local(d):
    """Aware datetime -> naive local time (resources.log and autopilot.log are local)."""
    return d.astimezone().replace(tzinfo=None) if d else None


def pct(values, q):
    if not values:
        return None
    s = sorted(values)
    k = max(0, min(len(s) - 1, int(round(q * (len(s) - 1)))))
    return s[k]


def mean(v):
    return sum(v) / len(v) if v else None


def num(x, nd=1):
    """Formats a number for tables ('-' for None)."""
    if x is None:
        return "-"
    if isinstance(x, float):
        return f"{x:.{nd}f}"
    return str(x)


def norm_message(msg):
    """One kind of warning: the message before its fields, numbers and hex as N."""
    key = re.sub(r'\s[a-z_]+=(\S+|"[^"]*"|\[[^\]]*\]).*$', "", msg)
    key = re.sub(r"0x[0-9a-fA-F]+", "N", key)
    key = re.sub(r"\b[0-9a-f]{8,}\b", "N", key)
    key = re.sub(r"\d+(\.\d+)?", "N", key)
    return key[:220]


# --- the run's files ---------------------------------------------------------------------------

class Run:
    """One run: paths and everything parsed from them (lazily)."""

    def __init__(self, tag):
        self.tag = tag
        self.dir = os.path.join(RUNS, tag)
        self.exists = os.path.isdir(self.dir)
        self.notes = []
        self.log = self.exe = self.exe_time = None
        for line in read_lines(os.path.join(self.dir, "run.txt")):
            k, _, v = line.partition(": ")
            if k == "log":
                self.log = v.strip()
            elif k == "exe":
                self.exe = v.strip()
            elif k == "exe time":
                self.exe_time = parse_iso(v)
        if not self.log or not os.path.isfile(self.log + ".err"):
            found = sorted(glob.glob(os.path.join(LOGS, f"run-{tag}-*.log")))
            found = [f for f in found if re.match(rf"run-{re.escape(tag)}-\d{{8}}-\d{{6}}\.log$", os.path.basename(f))]
            if found:
                if self.log:
                    self.notes.append(f"run.txt's log {self.log} has no .err; using {found[-1]}")
                self.log = found[-1]
        self.err = self.log + ".err" if self.log else None
        self.out = self.log + ".out" if self.log else None
        self._runlog = self._err = self._ap = self._res = self._summary = None

    # The launcher's own log: VICTORY_* settings, start, end, exit code.
    @property
    def runlog(self):
        if self._runlog is None:
            r = {"env": {}, "started": None, "ended": None, "ended_after": None, "exit": None, "lines": []}
            lines = read_lines(self.log) or read_lines(os.path.join(self.dir, "settings.txt"))
            for line in lines:
                r["lines"].append(line)
                m = re.match(r"^(VICTORY_[A-Z0-9_]+)=(.*)$", line)
                if m:
                    r["env"][m.group(1)] = m.group(2)
                elif line.startswith("started "):
                    r["started"] = parse_iso(line[8:])
                elif re.match(r"^ended after \d+ s", line):
                    r["ended_after"] = int(re.findall(r"\d+", line)[0])
                elif line.startswith("ended "):
                    r["ended"] = parse_iso(line[6:])
                elif line.startswith("exit code"):
                    r["exit"] = line[9:].strip()
            self._runlog = r
        return self._runlog

    @property
    def summary(self):
        if self._summary is None:
            self._summary = {}
            p = os.path.join(self.dir, "summary.json")
            if os.path.isfile(p):
                try:
                    with open(p, encoding="utf-8-sig") as f:
                        self._summary = json.load(f)
                except (OSError, ValueError):
                    self.notes.append("summary.json unreadable")
        return self._summary

    @property
    def started_local(self):
        return local(self.runlog["started"])

    @property
    def ended_local(self):
        return local(self.runlog["ended"])

    # The layer's output: one pass over the .err file.
    @property
    def errlog(self):
        if self._err is None:
            self._err = scan_err(self.err, self.out)
        return self._err

    @property
    def autopilot(self):
        if self._ap is None:
            self._ap = parse_autopilot(os.path.join(self.dir, "autopilot.log"), self.started_local)
        return self._ap

    def window(self):
        """The run's local time window (resources.log is local time)."""
        start = self.started_local or self.errlog["first_local"] or self.autopilot.get("first")
        end = self.ended_local or self.errlog["last_local"] or self.autopilot.get("last")
        if start and end:
            return start - timedelta(seconds=5), end + timedelta(seconds=5)
        return None, None

    @property
    def resources(self):
        if self._res is None:
            a, b = self.window()
            self._res = parse_resources(RESOURCES, a, b)
        return self._res

    @property
    def world_reached(self):
        ap = self.autopilot
        if ap["world_after"] is not None:
            return True
        if self.errlog["world_marker"]:
            return True
        return bool(self.summary.get("time_to_world_s")) and not ap["exists"]

    @property
    def load_seconds(self):
        if not self.world_reached:
            return None
        ap = self.autopilot
        if ap["world_after"] is not None:
            return ap["world_after"]
        return self.summary.get("time_to_world_s")


# --- .err scan --------------------------------------------------------------------------------

def scan_err(err, out=None):
    r = {
        "exists": bool(err and os.path.isfile(err)), "lines": 0, "first": None, "last": None, "first_local": None, "last_local": None,
        "ab": [], "memory": [], "warn": {}, "warn_example": {}, "warn_total": 0, "errors": 0,
        "title_count": 0, "title_last": [], "exceptions": [], "dumps": [], "removed": [], "markers": [],
        "world_marker": None, "title_module": None, "stalls": 0,
    }
    if not r["exists"]:
        return r
    cur = None          # the block frames go to: an exception or a dumped thread
    dump = None
    with open_text(err) as f:
        for n, raw in enumerate(f, 1):
            line = raw.rstrip("\r\n")
            if "\x1b" in line:
                line = RX_ANSI.sub("", line)
            r["lines"] = n
            if not line:
                continue
            c0 = line[0]
            if c0 == " ":
                m = RX_FRAME.match(line)
                if m:
                    if cur is not None:
                        cur["frames"].append(split_loc(m.group("loc")))
                    continue
                m = RX_THREAD.match(line)
                if m:
                    if dump is None:
                        dump = {"line": n, "count": None, "threads": []}
                        r["dumps"].append(dump)
                    cur = {"tid": int(m.group("tid")), "name": m.group("name"), "start": m.group("start"), "frames": [], "line": n}
                    dump["threads"].append(cur)
                    continue
                m = RX_TITLE_SAID.match(line)
                if m and cur is not None and "said" in cur and len(cur["said"]) < 12:
                    cur["said"].append(m.group(1))
                continue
            if c0 == "V" and line.startswith("VIctory: "):
                m = RX_EXC.match(line)
                if m:
                    cur = {"kind": m.group("kind"), "code": m.group("code"), "at": split_loc(m.group("at")), "desc": m.group("desc") or "",
                           "line": n, "text": line, "frames": [], "said": [], "ts": r["last"]}
                    r["exceptions"].append(cur)
                    continue
                m = RX_DUMP.match(line)
                if m:
                    dump = {"line": n, "count": int(m.group(1)), "threads": [], "ts": r["last"]}
                    r["dumps"].append(dump)
                    cur = None
                    continue
                if len(r["markers"]) < 200:
                    r["markers"].append(line)
                cur = None
                continue
            m = RX_LOG.match(line)
            if not m:
                if RX_REMOVED.search(line) and len(r["removed"]) < 20:
                    r["removed"].append(line[:400])
                continue
            ts = m.group("ts")
            if r["first"] is None:
                r["first"] = ts
            r["last"] = ts
            lvl, target, msg = m.group("lvl"), m.group("target"), m.group("msg")
            if lvl == "DEBUG" and target == "victory_gpu::frame":
                continue
            if target == "title":
                r["title_count"] += 1
                r["title_last"].append(f"{ts[11:19]} {msg[:300]}")
                if len(r["title_last"]) > 12:
                    r["title_last"].pop(0)
                continue
            if msg.startswith("A/B phase"):
                a = RX_AB.match(msg)
                if a:
                    r["ab"].append({"t": ts, "n": int(a.group("n")), "bits": int(a.group("bits")), "on": tuple(a.group("on").split()),
                                    "frames": int(a.group("frames")), "mean": float(a.group("mean")), "median": float(a.group("median")),
                                    "p90": float(a.group("p90")), "draws": float(a.group("draws"))})
                continue
            if msg.startswith("memory: "):
                a = RX_MEM.match(msg)
                if a:
                    r["memory"].append({"t": ts, **{k: int(a.group(k)) for k in ("private", "working", "free", "left")}})
                continue
            if r["title_module"] is None and "image mapped" in msg:
                a = RX_IMAGE.search(msg)
                if a:
                    r["title_module"] = a.group(1)
            if r["world_marker"] is None and msg.startswith("the first frame of 500 draws or more"):
                r["world_marker"] = ts
            if "crash tracing on" not in msg and RX_REMOVED.search(msg) and len(r["removed"]) < 20:
                r["removed"].append(f"{ts} {lvl} {target}: {msg[:400]}")
            if lvl in ("WARN", "ERROR"):
                if msg.startswith("the submission line stands still"):
                    r["stalls"] += 1
                key = f"{lvl} {target}: {norm_message(msg)}"
                r["warn"][key] = r["warn"].get(key, 0) + 1
                r["warn_total"] += 1
                if lvl == "ERROR":
                    r["errors"] += 1
                if key not in r["warn_example"]:
                    r["warn_example"][key] = f"{ts} {msg[:300]}"
    if r["title_module"] is None and out:
        for line in read_lines(out, 5):
            m = re.match(r"^build\s+\S+\s+\(([0-9a-f]{16})", line)
            if m:
                r["title_module"] = f"victory-image-{m.group(1)}.dll"
    r["first_local"] = local(parse_iso(r["first"]))
    r["last_local"] = local(parse_iso(r["last"]))
    return r


def split_loc(loc):
    """'module+0xrva' -> (module, rva int); anything else -> (loc, None)."""
    m = re.match(r"^(.*)\+0x([0-9a-fA-F]+)$", loc)
    return (m.group(1), int(m.group(2), 16)) if m else (loc, None)


def fmt_loc(loc, title_module=None):
    mod, rva = loc
    name = "title" if title_module and mod.lower() == title_module.lower() else mod
    return f"{name}+0x{rva:x}" if rva is not None else name


# --- autopilot.log ----------------------------------------------------------------------------

def parse_autopilot(path, started_local=None):
    r = {"exists": os.path.isfile(path), "captions": [], "events": [], "world_after": None, "world_at": None, "walk_start": None,
         "walk_seconds": None, "walk_done": None, "stuck": None, "exited": None, "stopped": None, "done_state": None, "first": None,
         "last": None, "log_errors": [], "pid": None}
    if not r["exists"]:
        return r
    day = (started_local or datetime.now()).date()
    prev = None

    def at(hms):
        nonlocal day, prev
        h, mi, s = hms.split(":")
        t = datetime.combine(day, datetime.min.time()) + timedelta(hours=int(h), minutes=int(mi), seconds=float(s))
        if prev and t < prev - timedelta(hours=12):
            day += timedelta(days=1)
            t += timedelta(days=1)
        prev = t
        return t

    for line in read_lines(path):
        m = RX_CAPTION.match(line)
        if m:
            t = at(m.group("t"))
            r["captions"].append({"t": t, "fps": float(m.group("fps")), "ms": float(m.group("ms")), "draws": float(m.group("draws")), "state": m.group("state")})
            r["first"] = r["first"] or t
            r["last"] = t
            continue
        m = RX_AP.match(line)
        if not m:
            continue
        t = at(m.group("t"))
        w = m.group("what")
        r["first"] = r["first"] or t
        r["last"] = t
        if w.startswith("press "):
            continue
        r["events"].append((t, w))
        if w.startswith("state: world reached after"):
            r["world_after"] = int(re.findall(r"\d+", w)[0])
            r["world_at"] = t
        elif re.match(r"state: walk \d+ s", w):
            r["walk_start"] = t
            r["walk_seconds"] = int(re.findall(r"\d+", w)[0])
        elif w.startswith("state: walk done"):
            r["walk_done"] = t
        elif w.startswith("state: stuck"):
            r["stuck"] = (t, w)
        elif w.startswith("victory.exe exited"):
            r["exited"] = (t, w)
        elif w.startswith("stopping victory.exe"):
            r["stopped"] = (t, w)
        elif w.startswith("autopilot done"):
            r["done_state"] = w
        elif w.startswith("victory.exe pid"):
            r["pid"] = w
        elif w.startswith("log ERROR") and len(r["log_errors"]) < 10:
            r["log_errors"].append(w)
    return r


def caption_stats(ap, after=60, min_draws=300):
    """Walk caption stats: all of the walk, and from `after` s into it (the first minute streams)."""
    caps = ap["captions"]
    base = ap["walk_start"] or ap["world_at"]
    states = ("walk",) if ap["walk_start"] else ("world", "settle")
    walk = [c for c in caps if c["state"] in states]
    late = [c for c in walk if base and (c["t"] - base).total_seconds() >= after]

    def stats(sel):
        if not sel:
            return None
        ms = [c["ms"] for c in sel]
        per = [c["ms"] / (c["draws"] / 1000) for c in sel if c["draws"] >= min_draws]
        best = min(sel, key=lambda c: c["ms"])
        worst = max(sel, key=lambda c: c["ms"])
        return {"n": len(sel), "median_ms": statistics.median(ms), "p10_ms": pct(ms, 0.10), "p90_ms": pct(ms, 0.90),
                "mean_ms": mean(ms), "median_draws": statistics.median([c["draws"] for c in sel]),
                "ms_per_1k": statistics.median(per) if per else None,
                "best": (best["ms"], best["draws"], best["t"].strftime("%H:%M:%S")), "worst": (worst["ms"], worst["draws"], worst["t"].strftime("%H:%M:%S"))}

    # draw-fit.ps1's line over walk and world captions: ms = a + b * draws/1000.
    pts = [(c["draws"] / 1000, c["ms"]) for c in caps if c["state"] in ("walk", "world") and c["draws"] >= min_draws and c["ms"] < 3000]
    fit = None
    if len(pts) >= 3:
        mx = mean([p[0] for p in pts])
        my = mean([p[1] for p in pts])
        sxx = sum((p[0] - mx) ** 2 for p in pts)
        b = sum((p[0] - mx) * (p[1] - my) for p in pts) / sxx if sxx > 0 else 0.0
        a = my - b * mx
        fit = {"n": len(pts), "fixed_ms": a, "per_1k_ms": b, "at_3k_ms": a + 3 * b}
    return {"states": states, "after_s": after, "all": stats(walk), "late": stats(late), "fit": fit}


# --- resources.log ----------------------------------------------------------------------------

def parse_resources(path, a, b):
    r = {"exists": os.path.isfile(path), "window": (a, b), "samples": [], "critical": [], "killed": [], "warnings": 0}
    if not r["exists"] or not a or not b:
        return r
    for line in read_lines(path):
        m = RX_RES.match(line)
        if not m:
            continue
        try:
            t = datetime.strptime(m.group(1), "%Y-%m-%d %H:%M:%S")
        except ValueError:
            continue
        if t < a or t > b:
            continue
        rest = m.group(2)
        if "KILLED" in rest:
            r["killed"].append(line)
            continue
        s = {"t": t}
        for key, rx in (("free", r"RAM: [\d.]+/[\d.]+ GB \(free: ([\d.]+)\)"), ("used", r"RAM: ([\d.]+)/"),
                        ("commit", r"Commit: ([\d.]+)/"), ("commit_free", r"Commit: [\d.]+/[\d.]+ GB \(free: ([\d.]+)\)"),
                        ("pagein", r"PageIn: ([\d.]+) MB/s"), ("gpu", r"GPU: ([\d.]+)%"),
                        ("ws", r"Victory: ([\d.]+) GB WS"), ("private", r"GB WS, ([\d.]+) GB Private"), ("modified", r"modified: ([\d.]+) GB")):
            mm = re.search(rx, rest)
            s[key] = float(mm.group(1)) if mm else None
        if "CRITICAL" in rest:
            r["critical"].append(line)
        elif "WARNING" in rest:
            r["warnings"] += 1
        r["samples"].append(s)
    return r


def resource_summary(res, spike_mbps=80):
    sm = res["samples"]

    def ext(key, fn):
        v = [s[key] for s in sm if s.get(key) is not None]
        return fn(v) if v else None

    spikes = [s for s in sm if (s.get("pagein") or 0) > spike_mbps]
    return {"samples": len(sm), "min_free_gb": ext("free", min), "max_commit_gb": ext("commit", max), "min_commit_free_gb": ext("commit_free", min),
            "max_pagein_mbps": ext("pagein", max), "pagein_spikes": len(spikes),
            "top_spikes": [(s["t"].strftime("%H:%M:%S"), s["pagein"], s.get("free")) for s in sorted(spikes, key=lambda s: -s["pagein"])[:3]],
            "max_ws_gb": ext("ws", max), "max_private_gb": ext("private", max),
            "critical": res["critical"], "killed": res["killed"], "warnings": res["warnings"]}


# --- A/B ----------------------------------------------------------------------------------------

def ab_aggregate(rows, world_draws=WORLD_DRAWS, since=None):
    """Per switch combination: frames-weighted mean ms and ms per 1k draws over the world's phases
    (from `since`, local time, on: the first minute in the world streams at 3-8x the cost per draw
    and would swamp the comparison), with the spread between phases; the hint compares each
    combination's difference from all-off with that spread (difference / standard error)."""
    used, early = [], 0
    prev_end = None
    for r in rows:
        end = local(parse_iso(r["t"]))
        start = prev_end or (end - timedelta(milliseconds=r["frames"] * r["mean"]) if end else None)
        prev_end = end
        if r["draws"] < world_draws or r["frames"] <= 0:
            continue
        if since and start and start < since:
            early += 1
            continue
        used.append(r)
    out = {"phases": len(rows), "used": len(used), "skipped": len(rows) - len(used), "early": early, "combos": [], "switches": [], "winner": None, "note": ""}
    if not used:
        out["note"] = ("no A/B phase in the world" + (f" after the first minute ({early} earlier)" if early else "")) if rows else "no A/B phases"
        return out
    switches = []
    for r in used:
        for s in r["on"]:
            if s not in switches:
                switches.append(s)

    def agg(sel, label, on):
        fr = sum(r["frames"] for r in sel)
        wm = sum(r["mean"] * r["frames"] for r in sel) / fr
        wd = sum(r["draws"] * r["frames"] for r in sel) / fr
        per = [r["mean"] / (r["draws"] / 1000) for r in sel]
        return {"label": label, "on": on, "phases": len(sel), "frames": fr, "mean_ms": wm, "draws": wd, "ms_per_1k": wm / (wd / 1000),
                "per_phase": per, "sd": statistics.stdev(per) if len(per) >= 2 else None}

    combos = {}
    for r in used:
        combos.setdefault(r["on"], []).append(r)
    res = [agg(v, " + ".join(k) if k else "all off", k) for k, v in sorted(combos.items(), key=lambda kv: (len(kv[0]), kv[0]))]
    base = next((c for c in res if not c["on"]), None)
    for c in res:
        c["diff"], c["z"], c["hint"] = compare(c, base)
    out["combos"] = res
    for s in switches:
        on = [r for r in used if s in r["on"]]
        off = [r for r in used if s not in r["on"]]
        if on and off:
            a, b = agg(on, f"{s} on", (s,)), agg(off, f"{s} off", ())
            d, z, h = compare(a, b)
            out["switches"].append({"switch": s, "on": a, "off": b, "diff": d, "z": z, "hint": h})
    win = min(res, key=lambda c: c["ms_per_1k"])
    out["winner"] = win
    if base is None:
        out["note"] = f"lowest ms per 1k draws: {win['label']} ({win['ms_per_1k']:.1f}); no all-off phase to compare with"
    elif win is base:
        out["note"] = f"all off is fastest ({base['ms_per_1k']:.1f} ms per 1k draws): no switch helps"
    else:
        out["note"] = (f"winner {win['label']}: {win['ms_per_1k']:.1f} vs {base['ms_per_1k']:.1f} ms per 1k draws all off "
                       f"({win['diff']:+.1f}, {win['hint']})")
    return out


def compare(c, base):
    if base is None or c is base:
        return None, None, ""
    d = c["ms_per_1k"] - base["ms_per_1k"]
    if c["sd"] is None or base["sd"] is None:
        return d, None, "too few phases to tell"
    se = (c["sd"] ** 2 / c["phases"] + base["sd"] ** 2 / base["phases"]) ** 0.5
    if se == 0:
        return d, None, "no spread"
    z = d / se
    hint = "likely real" if abs(z) >= 2 else ("weak" if abs(z) >= 1 else "within the noise")
    return d, z, f"{hint}, {abs(z):.1f}x the spread"


# --- symbols ------------------------------------------------------------------------------------

def exe_for_symbols(run):
    """The exe (with its PDB) the run used: (path, reliable, note). The diag/release exe is rebuilt in
    place, so its file time must match run.txt's 'exe time'; else an archived copy (build\\symbols,
    filled by run-queue.ps1) or another build folder with that time is looked for."""
    exe, when = run.exe, run.exe_time
    if not exe:
        return None, False, "run.txt names no exe"

    def mtime(p):
        try:
            return datetime.fromtimestamp(os.path.getmtime(p), tz=timezone.utc)
        except OSError:
            return None

    def same(p):
        t = mtime(p)
        return t is not None and when is not None and abs((t - when).total_seconds()) < 2 and os.path.isfile(os.path.splitext(p)[0] + ".pdb")

    if when is None:
        return (exe, False, "run.txt has no exe time: symbols unchecked") if os.path.isfile(exe) else (None, False, f"{exe} is gone")
    if os.path.isfile(exe) and same(exe):
        return exe, True, "the run's exe (file time matches run.txt)"
    cands = glob.glob(os.path.join(SYMBOL_ARCHIVE, "*", "victory.exe")) + glob.glob(r"C:\vt-*\*\victory.exe")
    for c in cands:
        if same(c):
            return c, True, f"the run's build found at {c}"
    t = mtime(exe)
    note = (f"{exe} was rebuilt since the run ({t.astimezone():%H:%M:%S} vs {when.astimezone():%H:%M:%S}) and no copy of the run's build "
            f"is kept: its symbols do not fit this run's offsets") if t else f"{exe} is gone"
    return (exe if os.path.isfile(exe) else None), False, note


def symbolize(exe, rvas):
    """victory.exe RVAs -> {rva: 'name file:line'} through addr2sym.py (dbghelp + the PDB)."""
    rvas = sorted(set(r for r in rvas if r is not None))
    if not exe or not rvas:
        return {}
    try:
        p = subprocess.run([sys.executable, "-I", os.path.join(TOOLS, "addr2sym.py"), exe] + [hex(r) for r in rvas],
                           capture_output=True, text=True, timeout=120)
    except (OSError, subprocess.TimeoutExpired) as e:
        return {r: f"(addr2sym failed: {e})" for r in rvas}
    out = {}
    for line in p.stdout.splitlines():
        m = re.match(r"^(0x[0-9a-fA-F]+): (.*)$", line)
        if m:
            text = m.group(2).strip()
            if text.startswith("? "):
                text = text[2:]
            out[int(m.group(1), 16)] = text
    return out


def short_src(text):
    """Trims source paths to the crate-relative part."""
    text = re.sub(r"^.*?[\\/]crates[\\/]", "crates/", text)
    text = re.sub(r"^/rustc/[0-9a-f]+/", "rustc/", text)
    return text.replace("\\", "/")


# --- the faulting thread and title-main --------------------------------------------------------

def faulting(errlog):
    """The exception that ended the run: the last unhandled one, else the last first-chance one."""
    exc = errlog["exceptions"]
    un = [e for e in exc if e["kind"] == "unhandled"]
    return un[-1] if un else (exc[-1] if exc else None)


def thread_in_dumps(errlog, name):
    """The named thread in each dump (last dump last)."""
    out = []
    for d in errlog["dumps"]:
        for t in d["threads"]:
            if t["name"] == name:
                out.append((d, t))
    return out


# --- signatures ---------------------------------------------------------------------------------

def load_signatures(path=SIGNATURES):
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    sigs = sorted(data["signatures"], key=lambda s: s.get("priority", 100))
    return data, sigs


def loc_matches(loc, want, title_module):
    """loc (module, rva) against 'title+0x56a0d' / 'victory.exe+0x...' / 'ntdll.dll' (module only)."""
    mod, rva = loc
    wm, wr = split_loc(want)
    name = "title" if title_module and mod.lower() == title_module.lower() else mod
    if wm.lower() != name.lower():
        return False
    return wr is None or wr == rva


def check(cond, run, ctx):
    """Evaluates one signature's conditions (all must hold); returns (ok, evidence list)."""
    ev = []
    el = run.errlog
    tm = el["title_module"]
    exc = faulting(el)
    for key, val in cond.items():
        if key == "world":
            if bool(run.world_reached) != bool(val):
                return False, ev
            ev.append("world reached" if val else "no world")
        elif key == "resources_regex":
            rx = re.compile(val)
            hit = [l for l in run.resources["killed"] + run.resources["critical"] if rx.search(l)]
            if not hit:
                return False, ev
            ev.append("resources.log: " + hit[-1])
            if "KILLED" in hit[-1]:
                # The sample that triggered it: the last CRITICAL line at or before the kill.
                crit = [l for l in run.resources["critical"] if l[:19] <= hit[-1][:19]]
                if crit:
                    ev.append("resources.log: " + crit[-1][:400])
        elif key == "log_regex":
            rx = re.compile(val, re.I)
            hits = [l for l in el["removed"] if rx.search(l)] if cond.get("log_source") == "removed" else None
            if hits is None:
                hits = grep(run.err, rx, cond.get("log_exclude"), 3)
            if not hits:
                return False, ev
            ev.append("log: " + hits[0][:300])
        elif key == "autopilot_regex":
            rx = re.compile(val)
            hits = [f"{t:%H:%M:%S} {w}" for t, w in run.autopilot["events"] if rx.search(w)]
            if not hits:
                return False, ev
            ev.append("autopilot: " + hits[0])
        elif key == "runlog_regex":
            rx = re.compile(val)
            hits = [l for l in run.runlog["lines"] if rx.search(l)]
            if not hits:
                return False, ev
            ev.append("run log: " + hits[0])
        elif key == "setting":
            if val not in run.runlog["env"]:
                return False, ev
            ev.append(f"{val}={run.runlog['env'][val]}")
        elif key == "marker_regex":
            rx = re.compile(val)
            hits = [l for l in el["markers"] if rx.search(l)]
            if not hits:
                return False, ev
            ev.append("log: " + hits[0])
        elif key == "exception":
            if val == "unhandled":
                if not exc or exc["kind"] != "unhandled":
                    return False, ev
            elif not exc:
                return False, ev
            ev.append(f"line {exc['line']}: {exc['text'][:250]}")
        elif key == "exception_at":
            hits = [e for e in el["exceptions"] if loc_matches(e["at"], val, tm)]
            if not hits:
                return False, ev
            e = next((x for x in hits if x["kind"] == "unhandled"), hits[0])
            ev.append(f"line {e['line']}: {e['text'][:250]}")
        elif key == "exception_module_regex":
            if not exc or not re.search(val, exc["at"][0], re.I):
                return False, ev
        elif key == "frames_module_regex":
            if not exc or not any(re.search(val, fr[0], re.I) for fr in exc["frames"]):
                return False, ev
        elif key == "frames_before_layer_regex":
            # Every frame above the first victory.exe one is in a module matching this (system DLLs):
            # the fault came from the layer's own code calling the system, not from the title.
            fr = exc["frames"] if exc else []
            first = next((i for i, f in enumerate(fr) if f[0].lower() == "victory.exe"), None)
            if first is None or not all(re.search(val, f[0], re.I) for f in fr[:first]):
                return False, ev
            ev.append("faulting stack: " + " < ".join(fmt_loc(f, tm) for f in fr[: first + 1]))
        elif key == "symbols_regex":
            if not exc:
                return False, ev
            syms = ctx.symbols_for(exc)
            if not ctx.reliable:
                return False, ev + ["symbols unreliable: " + ctx.exe_note]
            rx = re.compile(val)
            hits = [f"#{i} {fmt_loc(fr, tm)} {short_src(syms.get(fr[1], ''))}" for i, fr in enumerate(exc["frames"])
                    if fr[0].lower() == "victory.exe" and rx.search(syms.get(fr[1], ""))]
            if not hits:
                return False, ev
            ev.append("symbol: " + hits[0])
        elif key == "thread":
            pass  # with frames_in_order
        elif key == "frames_in_order":
            name = cond.get("thread", "title-main")
            top = cond.get("within_top", 8)
            found = None
            for d, t in reversed(thread_in_dumps(el, name)):
                fr = t["frames"][:top]
                for i in range(len(fr) - len(val) + 1):
                    if all(loc_matches(fr[i + j], val[j], tm) for j in range(len(val))):
                        found = (d, t, i)
                        break
                if found:
                    break
            if not found:
                return False, ev
            d, t, i = found
            ev.append(f"thread dump (line {d['line']}): {name} at " + " < ".join(fmt_loc(f, tm) for f in t["frames"][: i + len(val)]))
        elif key in ("within_top", "log_exclude", "log_source"):
            pass
        elif key == "exit_code":
            if str(run.runlog["exit"]) not in [str(v) for v in val]:
                return False, ev
            ev.append(f"exit code {run.runlog['exit']}")
        elif key == "no_dump":
            if bool(el["dumps"]) == bool(val):
                return False, ev
        else:
            raise ValueError(f"unknown signature condition {key!r}")
    return True, ev


def grep(path, rx, exclude=None, most=3):
    out = []
    ex = re.compile(exclude, re.I) if exclude else None
    if not path or not os.path.isfile(path):
        return out
    with open_text(path) as f:
        for line in f:
            if rx.search(line) and not (ex and ex.search(line)):
                out.append(RX_ANSI.sub("", line.rstrip()))
                if len(out) >= most:
                    break
    return out


class Context:
    """Symbols on demand, once per run."""

    def __init__(self, run):
        self.run = run
        self.exe, self.reliable, self.exe_note = exe_for_symbols(run)
        self._syms = {}

    def symbols_for(self, block):
        want = [fr[1] for fr in block["frames"] if fr[0].lower() == "victory.exe" and fr[1] not in self._syms]
        if block.get("at") and block["at"][0].lower() == "victory.exe":
            want.append(block["at"][1])
        if want and self.exe:
            self._syms.update(symbolize(self.exe, want))
        return self._syms


def classify(run, sigs_path=SIGNATURES):
    """Every signature that matches, by priority; the first is the verdict."""
    data, sigs = load_signatures(sigs_path)
    ctx = Context(run)
    matches = []
    for s in sigs:
        ok, ev = check(s.get("match", {}), run, ctx)
        heuristic = None
        if not ok and s.get("fallback") and not ctx.reliable:
            ok, ev2 = check(s["fallback"], run, ctx)
            if ok:
                ev = ev2
                heuristic = "matched by its fallback (the exe was rebuilt since the run, so its symbols could not be checked): " + ctx.exe_note
        if ok:
            # 'world reached' / 'no world' says least: last, so the first evidence line is the telling one.
            ev = [e for e in ev if e not in ("world reached", "no world")] + [e for e in ev if e in ("world reached", "no world")]
            matches.append({"id": s["id"], "name": s["name"], "class": s.get("class", "fail"), "priority": s.get("priority", 100),
                            "evidence": ev, "heuristic": heuristic, "advice": s.get("advice", "")})
    if not matches:
        matches.append({"id": "unknown", "name": "Unknown ending", "class": "unknown", "priority": 999, "evidence": [ending_evidence(run)],
                        "heuristic": None, "advice": "no signature matched: see the stacks below; add one to tools/triage-signatures.json"})
    return matches, ctx


def ending_evidence(run):
    rl, ap = run.runlog, run.autopilot
    bits = []
    if rl["exit"] is not None:
        bits.append(f"exit code {rl['exit']}")
    if rl["ended_after"]:
        bits.append(f"ended after {rl['ended_after']} s")
    if ap["exited"]:
        bits.append(f"autopilot: {ap['exited'][1]}")
    if not run.errlog["exists"]:
        bits.append("no .err run log")
    return "; ".join(bits) or "nothing recorded"


def stack_lines(block, ctx, title_module, limit=24, force=False):
    """A stack as text lines, victory.exe frames symbolized when the run's build is at hand (a rebuilt
    exe's symbols would name the wrong functions, so they are left out unless forced)."""
    layer = any(f[0].lower() == "victory.exe" for f in block["frames"])
    syms = ctx.symbols_for(block) if layer and (ctx.reliable or force) else {}
    out = []
    for i, fr in enumerate(block["frames"][:limit]):
        s = syms.get(fr[1], "") if fr[0].lower() == "victory.exe" else ""
        out.append(f"#{i:<2} {fmt_loc(fr, title_module):<34} {short_src(s)}".rstrip())
    if len(block["frames"]) > limit:
        out.append(f"... {len(block['frames']) - limit} more")
    return out
