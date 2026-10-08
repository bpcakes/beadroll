"""Import every local Beads export into a scratch store and export it again; records must come back identical.
Reads ~/Documents/*/.beads/issues.jsonl only; writes nothing outside the scratch store."""
import glob, json, os, subprocess, sys, time
BP = os.path.expanduser("~/Documents/beadroll/beadroll.py")
env = {k: v for k, v in os.environ.items() if not k.startswith(("CLAUDE", "CODEX", "BP_"))}
env.update(BEAD_HOME=os.environ.get("RT_HOME") or os.path.join(os.environ.get("TMPDIR", "/tmp"), "bp-roundtrip-home"), BEAD_AGENT="human")
def norm(rec):
    rec = dict(rec); rec.pop("source_repo_path", None)
    if "labels" in rec: rec["labels"] = sorted(rec["labels"])
    if "dependencies" in rec: rec["dependencies"] = sorted(rec["dependencies"], key=lambda d: (d["depends_on_id"], d["type"]))
    return rec
total = same = 0; diffs = []; rows = []
for path in sorted(glob.glob(os.path.expanduser("~/Documents/*/.beads/issues.jsonl"))):
    repo = path.split("/")[-3]
    if not os.path.isdir(os.path.dirname(os.path.dirname(path)) + "/.git"): continue
    src = [json.loads(l) for l in open(path) if l.strip()]
    if not src: continue
    e = dict(env, BEAD_WORKSPACE="rt-" + repo)
    t = time.time()
    p = subprocess.run([BP, "init", "--prefix", "x", "--import", path, "--json"], env=e, capture_output=True, text=True)
    t_imp = time.time() - t
    if p.returncode: print(repo, "IMPORT FAILED", p.stderr[:300]); continue
    t = time.time(); out = subprocess.run([BP, "export"], env=e, capture_output=True, text=True); t_exp = time.time() - t
    got = {r["id"]: r for r in map(json.loads, out.stdout.splitlines())}
    ok = 0
    for rec in src:
        total += 1
        a, b = norm(rec), norm(got.get(rec["id"], {}))
        if a == b: ok += 1; same += 1
        elif len(diffs) < 12:
            keys = sorted(k for k in set(a) | set(b) if a.get(k) != b.get(k))
            diffs.append((repo, rec["id"], keys, {k: (str(a.get(k))[:70], str(b.get(k))[:70]) for k in keys[:2]}))
    t = time.time(); subprocess.run([BP, "ready", "--json"], env=e, capture_output=True); t_ready = time.time() - t
    seg = glob.glob(os.path.join(env["BEAD_HOME"], f"rt-{repo}/events/*/*.jsonl"))
    rows.append((repo, len(src), ok, round(t_imp*1000), round(t_exp*1000), round(t_ready*1000), sum(os.path.getsize(s) for s in seg)//1024, os.path.getsize(path)//1024))
print(f"{'tracker':30s} issues  identical  import_ms export_ms ready_ms  journal_KB  beads_KB")
for r in rows: print(f"{r[0]:30s} {r[1]:5d}  {r[2]:5d}      {r[3]:6d}  {r[4]:6d}  {r[5]:6d}   {r[6]:7d}  {r[7]:7d}")
print(f"\nTOTAL {same}/{total} records identical after import -> export (ignoring source_repo_path)")
for d in diffs: print("DIFF", d)
