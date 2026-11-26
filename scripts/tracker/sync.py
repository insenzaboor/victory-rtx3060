"""Syncs issues and the project board with docs/data/ports.json (labels, new issues, board fields)."""
import json, os, subprocess
OWNER, REPO, NUM = "insenzaboor", "insenzaboor/victory-rtx3060", "2"
p = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "docs", "data", "ports.json")
d = json.load(open(p, encoding="utf-8"))
def gh(*a):
    r = subprocess.run(["gh", *a], capture_output=True, text=True, encoding="utf-8")
    if r.returncode: print("ERR", " ".join(a[:4]), r.stderr.strip()[:200]); return None
    return r.stdout.strip()
OPEN = {"default-untested", "ready", "in-progress", "planned"}
STAT = {"shipped": "Shipped", "default-untested": "On (untested)", "ready": "Ready to test", "in-progress": "In progress", "planned": "Planned", "no-gain": "No gain (off)", "not-applicable": "Not applicable"}
BOARD = {"shipped": "Done", "no-gain": "Done", "not-applicable": "Done", "ready": "In Progress", "default-untested": "In Progress", "in-progress": "In Progress", "planned": "Todo"}
cats = {k: v.replace(", ", " / ") for k, v in d["categories"].items()}
proj = json.loads(gh("project", "view", NUM, "--owner", OWNER, "--format", "json"))
pid = proj["id"]
F = {f["name"]: f for f in json.loads(gh("project", "field-list", NUM, "--owner", OWNER, "--format", "json", "--limit", "50"))["fields"]}
items = json.loads(gh("project", "item-list", NUM, "--owner", OWNER, "--format", "json", "--limit", "500"))["items"]
by_title = {i.get("title") or i.get("content", {}).get("title"): i for i in items}
def opt(f, n): return next(o["id"] for o in F[f]["options"] if o["name"] == n)
for it in d["items"]:
    labels = [f"cat:{it['category']}", f"status:{it['status']}"] + (["needs-game-run"] if it["status"] in ("ready", "default-untested") else [])
    if it.get("issue"):
        cur = json.loads(gh("issue", "view", str(it["issue"]), "--repo", REPO, "--json", "labels,title"))
        old = [l["name"] for l in cur["labels"] if l["name"].startswith("status:") or l["name"] == "needs-game-run"]
        want = [l for l in labels if l.startswith("status:") or l == "needs-game-run"]
        if set(old) != set(want):
            gh("issue", "edit", str(it["issue"]), "--repo", REPO, "--remove-label", ",".join(old) or "none", "--add-label", ",".join(want))
            print("relabelled", it["issue"], want)
    elif it["status"] in OPEN:
        body = f"**Category:** {d['categories'][it['category']]}\n**Status:** {d['statuses'][it['status']]}\n**Source:** {it['source']}" + \
               (f"\n**Switch:** `{it['switch']}`" if it.get("switch") else "") + (f"\n**Commit:** {it['commit']}" if it.get("commit") else "") + \
               (f"\n**Effect:** {it['effect']}" if it.get("effect") else "") + "\n\n### To close\n- [ ] Measured in a game run\n- [ ] Screenshots checked\n- [ ] ports.json updated, tracker rebuilt"
        url = gh("issue", "create", "--repo", REPO, "--title", it["title"], "--body", body, "--label", ",".join(labels))
        if url: it["issue"] = int(url.rsplit("/", 1)[-1]); print("issue", it["issue"], it["title"])
    card = by_title.get(it["title"])
    if not card:
        if it.get("issue"):
            card = json.loads(gh("project", "item-add", NUM, "--owner", OWNER, "--url", f"https://github.com/{REPO}/issues/{it['issue']}", "--format", "json"))
        else:
            card = json.loads(gh("project", "item-create", NUM, "--owner", OWNER, "--title", it["title"], "--body", f"**Source:** {it['source']}\n\n**Effect:** {it.get('effect','')}", "--format", "json"))
        print("card", it["title"])
    elif (card.get("port status") or card.get("Port status")) == STAT[it["status"]]:
        continue
    iid = card["id"]
    for f, n in (("Port status", STAT[it["status"]]), ("Category", cats[it["category"]]), ("Status", BOARD[it["status"]])):
        gh("project", "item-edit", "--id", iid, "--project-id", pid, "--field-id", F[f]["id"], "--single-select-option-id", opt(f, n))
    for f, v in (("Source", it["source"]), ("Switch", it.get("switch", "")), ("Effect", it.get("effect", "")), ("Commit", it.get("commit", ""))):
        if v: gh("project", "item-edit", "--id", iid, "--project-id", pid, "--field-id", F[f]["id"], "--text", v[:1000])
json.dump(d, open(p, "w", encoding="utf-8", newline="\n"), indent=2, ensure_ascii=False)
# Draft cards whose item was renamed or removed (issue cards are never deleted here).
titles = {i["title"] for i in d["items"]}
for c in json.loads(gh("project", "item-list", NUM, "--owner", OWNER, "--format", "json", "--limit", "500"))["items"]:
    if c.get("content", {}).get("type") == "DraftIssue" and c.get("title") not in titles:
        gh("project", "item-delete", NUM, "--owner", OWNER, "--id", c["id"])
        print("removed stale card", c.get("title"))
print("synced")
