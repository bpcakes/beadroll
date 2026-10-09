"""Randomised multi-machine models. Slow; run on demand.

Gathered from: first independent audit (AUDIT), 2026-10-06; second independent audit (REAUDIT), 2026-10-06. Docstrings and names that cite
finding ids (A.., R.., N.., C.., K.., P.., D..) refer to docs/FINDINGS.md."""
import argparse
import collections
import concurrent.futures
import copy
import importlib.machinery
import importlib.util
import itertools
import json
import os
import random
import resource
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import uuid
from pathlib import Path
from unittest import mock
from unittest.mock import patch

import tests.support as support
from tests.support import *  # noqa: F401,F403  (BEAD, Machine, Sandbox, load_module, git, code)


# ---- from test_audit.py: first independent audit (AUDIT), 2026-10-06
def audit_model_run(repo, operations=2400, seed=20261006, output=None):
    # `poc` is tests.support (module level); `repo` is kept for the signature the reports cite
    rnd = random.Random(seed)
    sandbox = support.Sandbox()
    machines = [support.Machine(sandbox, f"soak{i}") for i in range(6)]
    root = machines[0]
    root.bead("init", "--prefix", "soak")
    keys = [root.bead("create", f"seed issue {i}")["id"] for i in range(24)]
    for m in machines:
        m.bead("sync")
    counts, errors = collections.Counter(), collections.Counter()
    model, model_owner, generations, accepted = {}, {}, {}, set()
    partitions, checks, transcript = {}, [], []
    started = time.monotonic()
    module = support.load_module()

    def full_check(step):
        for m in machines * 2:
            result = m.bead("sync")
            assert not result["forked_journals"], result
            assert not result["waiting_events"], result
        exports, views, readies = [], [], []
        for m in machines:
            exports.append(m.export())
            info = m.bead("info")
            assert not info["waiting_events"] and not info["integrity"], info
            details = m.bead("show", *keys)
            views.append(json.dumps(details, sort_keys=True))
            readies.append(m.ready())
            for d in details:
                current = d.get("claim")
                assert (current["id"] if current else None) == model.get(d["id"]), (step, m.name, d, model)
                if current:
                    assert current["generation"] == generations[d["id"]], (step, current, generations)
                expect_ready = d["status"] == "open" and not d["blocked"] and "status" not in d.get("conflicts", {})
                assert (d["id"] in readies[-1]) == expect_ready, (step, d)
            old_home = os.environ.get("BEAD_HOME")
            os.environ["BEAD_HOME"] = str(m.home)
            try:
                ws = module.Workspace(m.store().name, None, None, {})
                state = module.replay(module.load_journal(ws))
                assert accepted <= set(state.journal.events), (step, "lost accepted event")
                realized = {c["id"] for i in state.issues.values() for c in i["claim_history"]}
                realized |= {i["claim"]["id"] for i in state.issues.values() if i["claim"]}
                assert accepted <= realized, (step, "accepted claim missing from current/history", accepted-realized)
            finally:
                if old_home is None:
                    os.environ.pop("BEAD_HOME", None)
                else:
                    os.environ["BEAD_HOME"] = old_home
        assert len(set(exports)) == 1, (step, "exports diverged")
        assert len(set(views)) == 1, (step, "details diverged")
        assert all(r == readies[0] for r in readies), (step, "readiness diverged")
        check = {"step": step, "issues": len(keys), "accepted_claims": len(accepted), "events": info["events"]}
        checks.append(check)
        print(json.dumps({"checkpoint": check}), flush=True)

    actions = ["show", "update", "list", "ready", "close", "create", "dep", "comment", "claim", "release", "takeover", "sync"]
    weights = [32,12,9,6,5,5,5,4,6,4,3,9]
    try:
        for step in range(1, operations + 1):
            if step % 160 == 1:
                chosen = rnd.randrange(6)
                partitions[chosen] = step + rnd.randrange(40, 100)
            mi = rnd.randrange(6)
            m = machines[mi]
            offline = partitions.get(mi, 0) >= step
            action = rnd.choices(actions, weights)[0]
            key = rnd.choice(keys)
            args = []
            if action == "show": args = ["show", key]
            elif action == "list": args = ["list", "--all", "--limit", "0"]
            elif action == "ready": args = ["ready"]
            elif action == "update":
                args = ["update", key, "--notes", f"step {step} machine {mi}", "--priority", str(rnd.randrange(5))]
                args += [rnd.choice(["--add-label", "--remove-label"]), rnd.choice(["alpha", "beta", "gamma"])]
            elif action == "create": args = ["create", f"generated {step}"]
            elif action == "close":
                # Restrict random closes to issues without a confirmed model owner;
                # late close/takeover and forced transitions have separate regressions.
                if key in model: args = ["show", key]
                else: args = ["close", key, "--offline"]
            elif action == "dep":
                target = rnd.choice(keys)
                args = ["dep", rnd.choice(["add", "remove"]), key, target]
            elif action == "comment": args = ["comments", "add", key, f"synthetic note {step}"]
            elif action == "sync": args = ["info"] if offline else ["sync"]
            elif action == "claim": args = ["claim", key] + (["--offline"] if offline else [])
            elif action == "release":
                args = ["show", key] if offline else ["release", key]
            elif action == "takeover":
                args = ["show", key] if offline else ["takeover", key, "--reason", "synthetic model test"]
            counts[args[0]] += 1
            proc = m.run(*args, "--json")
            row = {"step": step, "machine": mi, "partitioned": offline, "args": args, "exit": proc.returncode}
            if proc.returncode:
                try: code = json.loads(proc.stderr)["error"]["code"]
                except Exception: raise AssertionError((step, proc.stderr))
                errors[code] += 1
                row["error"] = code
                assert code in {"ISSUE_NOT_FOUND", "VALIDATION", "STATUS_CONFLICT", "ALREADY_CLAIMED", "NOT_OWNER", "CLAIMED_BY_OTHER", "CYCLE"}, row
            else:
                result = json.loads(proc.stdout)
                if args[0] == "create": keys.append(result["id"])
                elif args[0] in {"claim", "takeover"} and not offline:
                    held = result[0]["claim"]
                    if held["id"] != model.get(key):
                        assert held["generation"] == generations.get(key, 0) + 1, (step, held, generations)
                    generations[key] = held["generation"]
                    model[key] = held["id"]
                    model_owner[key] = mi
                    accepted.add(held["id"])
                elif args[0] == "release" and model_owner.get(key) == mi:
                    model.pop(key, None)
                    model_owner.pop(key, None)
            transcript.append(row)
            if step % 300 == 0 or step == operations:
                full_check(step)
        summary = {"status": "passed", "operations": operations, "machines": 6, "seed": seed,
                   "seconds": round(time.monotonic()-started, 3), "counts": dict(counts), "errors": dict(errors),
                   "checks": checks, "accepted_online_claim_events": len(accepted), "scratch": str(sandbox.root),
                   "limits": "Sequential randomized CLI operations; partitions simulate withheld sync. Online claim races tested separately. Readiness oracle checks command consistency, not independent graph implementation."}
    except Exception as error:
        summary = {"status": "failed", "error": repr(error), "operations_completed": len(transcript), "scratch": str(sandbox.root), "counts": dict(counts), "errors": dict(errors)}
        raise
    finally:
        if output:
            Path(output).write_text(json.dumps(summary, indent=2)+"\n")
            Path(output).with_suffix(".commands.jsonl").write_text("".join(json.dumps(t)+"\n" for t in transcript))
        print(json.dumps(summary), flush=True)
    return summary


# ---- from test_audit.py: first independent audit (AUDIT), 2026-10-06
class RootAuditStress(unittest.TestCase):
    def test_six_machine_2400_operation_reference_model(self):
        result = audit_model_run(Path(__file__).resolve().parent, 2400, 20261006)
        self.addCleanup(shutil.rmtree, result["scratch"], ignore_errors=True)
        self.assertEqual(result["status"], "passed")
        self.assertEqual(len(result["checks"]), 8)
        self.assertEqual(result["operations"], 2400)


# ---- from test_reaudit.py: second independent audit (REAUDIT), 2026-10-06
def run_concurrent_stress(repo, output, waves=10, per_wave=50, seed=20261007):
    # `poc` is tests.support (module level); `repo` is kept for the signature the reports cite
    box=support.Sandbox()
    machines=[support.Machine(box,f'wave{i}') for i in range(6)]
    machines[0].bead('init','--prefix','wave')
    editable=[machines[0].bead('create',f'editable {i}')["id"] for i in range(24)]
    owned=[machines[0].bead('create',f'ownership {i}')["id"] for i in range(12)]
    for m in machines:m.bead('sync')
    allkeys=editable+owned
    histories=[]
    workerclaims=[{} for _ in machines]
    generations={}
    counts=collections.Counter();errors=collections.Counter();checks=[];transcript=[]
    start=time.monotonic()
    allowed={'ALREADY_CLAIMED','NOT_OWNER','CLAIMED_BY_OTHER','VALIDATION','STATUS_CONFLICT','CYCLE','ISSUE_NOT_FOUND','SYNC_CONFLICT','REMOTE_UNREACHABLE'}
    bead=support.load_module()

    def worker(mi,wave):
        m=machines[mi];rnd=random.Random(seed+wave*101+mi*907)
        partitioned=mi in {wave%6,(wave+2)%6}
        rows=[];newkeys=[];receipts=[]
        for step in range(per_wave):
            action=rnd.choices(['show','list','ready','edit','comment','create','dep','status','claim','release','takeover','sync'],[28,8,6,14,6,5,5,5,8,5,4,6])[0]
            key=rnd.choice(editable)
            if action=='show':args=['show',rnd.choice(allkeys)]
            elif action=='list':args=['list','--all','--limit','0']
            elif action=='ready':args=['ready']
            elif action=='edit':args=['update',key,'--notes',f'{wave}/{mi}/{step}','--priority',str(rnd.randrange(5)),rnd.choice(['--add-label','--remove-label']),'tag'+str(rnd.randrange(4))]
            elif action=='comment':args=['comments','add',key,f'synthetic {wave}/{mi}/{step}']
            elif action=='create':args=['create',f'new {wave}/{mi}/{step}']
            elif action=='dep':args=['dep',rnd.choice(['add','remove']),key,rnd.choice(editable)]
            elif action=='status':args=['update',key,'--status',rnd.choice(['open','closed','deferred','blocked'])]
            elif action in {'claim','release','takeover'}:
                key=rnd.choice(owned)
                args=['show',key] if partitioned else [action,key]+(['--reason','concurrency audit'] if action=='takeover' else [])
            else:args=['info'] if partitioned else ['sync']
            p=m.run(*args,'--json')
            row={'wave':wave,'machine':mi,'step':step,'partitioned':partitioned,'args':args,'exit':p.returncode}
            if p.returncode:
                try:row['error']=support.code(p)
                except Exception:raise AssertionError(('unstructured error',row,p.stderr))
                assert row['error'] in allowed,(row,p.stderr)
            else:
                val=json.loads(p.stdout)
                if args[0]=='create':newkeys.append(val['id'])
                elif args[0] in {'claim','takeover'}:
                    held=val[0].get('claim')
                    assert held,(row,val)
                    workerclaims[mi][key]=held
                    receipts.append({'op':args[0],'key':key,'generation':held['generation'],'owner':held['owner'],'id':held['id']})
                elif args[0]=='release':
                    held=workerclaims[mi].get(key)
                    assert held,('released without own accepted claim receipt',row)
                    receipts.append({'op':'release','key':key,'generation':held['generation'],'owner':held['owner'],'id':held['id']})
            rows.append(row)
        return rows,newkeys,receipts

    summary=None
    try:
        for wave in range(waves):
            with concurrent.futures.ThreadPoolExecutor(6) as pool:
                results=list(pool.map(lambda mi:worker(mi,wave),range(6)))
            for rows,newkeys,receipts in results:
                transcript+=rows;editable+=newkeys;allkeys+=newkeys;histories+=receipts
                counts.update(r['args'][0] for r in rows)
                errors.update(r['error'] for r in rows if 'error' in r)
            for m in machines*2:
                r=m.bead('sync')
                assert r['waiting_events']==0 and not r['forked_journals'] and r['integrity']==0,r
            exports=[m.export() for m in machines]
            assert len(set(exports))==1,('export divergence',wave)
            records=[json.loads(l) for l in exports[0].splitlines()]
            keys=sorted(r['id'] for r in records)
            details=[m.bead('show',*keys) for m in machines]
            assert len({json.dumps(d,sort_keys=True) for d in details})==1,('detail divergence',wave)
            expected={}
            for key in owned:
                got=[r for r in histories if r['key']==key]
                if not got:expected[key]=None;continue
                g=max(r['generation'] for r in got)
                gen=[r for r in got if r['generation']==g]
                claims={r['id']:r for r in gen if r['op']!='release'}
                assert len(claims)==1,('same generation multiple owners',key,gen)
                current=next(iter(claims.values()))
                expected[key]=None if any(r['op']=='release' for r in gen) else current
                assert g>=generations.get(key,0)
                generations[key]=g
            for d in details[0]:
                if d['id'] in expected:
                    want=expected[d['id']]
                    assert (d.get('claim') or {}).get('id')==(want or {}).get('id'),('ownership model mismatch',wave,d,want)
            # Independent blocker oracle over exported/show graph (all generated issues are tasks).
            byid={d['id']:d for d in details[0]}
            def done(d):return d['status'] in {'closed','tombstone'} and 'status' not in d.get('conflicts',{})
            expectedready=set()
            for key,d in byid.items():
                blocked=any(e['dependency_type']=='blocks' and e['id'] in byid and not done(byid[e['id']]) for e in d['dependencies'])
                assert d['blocked']==blocked,(wave,key,'blocker oracle mismatch')
                if d['status']=='open' and not blocked and not d.get('claim') and 'status' not in d.get('conflicts',{}):expectedready.add(key)
            for m in machines:
                assert m.ready()==expectedready,(wave,m.name,'ready mismatch')
                info=m.bead('info');assert not info['integrity'] and not info['waiting_events'],info
                assert not json.loads((m.store()/'unconfirmed.json').read_text()) if (m.store()/'unconfirmed.json').exists() else True
            accepted={r['id'] for r in histories if r['op']!='release'}
            with __import__('unittest.mock',fromlist=['patch']).patch.dict(os.environ,{'BEAD_HOME':str(machines[0].home)}):
                st=bead.view(bead.Workspace(machines[0].store().name,None,None,{}))
                realized={c['id']for i in st.issues.values()for c in i['claim_history']}|{i['claim']['id'] for i in st.issues.values() if i['claim']}
                assert accepted<=realized,('accepted claim disappeared',accepted-realized)
            c={'wave':wave,'operations':len(transcript),'issues':len(keys),'events':info['events'],'accepted_claim_ids':len(accepted),'max_generation':max(generations.values(),default=0)}
            checks.append(c);print(json.dumps({'checkpoint':c}),flush=True)
        summary={'status':'passed','machines':6,'operations':len(transcript),'seed':seed,'waves':waves,'per_worker_wave':per_wave,'counts':dict(counts),'domain_errors':dict(errors),'checks':checks,'seconds':time.monotonic()-start,'scratch':str(box.root)}
    except Exception as exc:
        summary={'status':'failed','error':repr(exc),'operations_logged':len(transcript),'checks':checks,'scratch':str(box.root)}
        raise
    finally:
        if output:
            Path(output).write_text(json.dumps(summary,indent=2)+'\n')
            Path(output).with_suffix('.commands.jsonl').write_text(''.join(json.dumps(r)+'\n'for r in transcript))
        print(json.dumps(summary),flush=True)
        box.cleanup()
    return summary


# ---- from test_reaudit.py: second independent audit (REAUDIT), 2026-10-06
class ConcurrentReaudit(unittest.TestCase):
    def test_six_machine_3000_concurrent_commands_with_partitions(self):
        with tempfile.TemporaryDirectory(prefix="bead-concurrent-reaudit-") as tmp:
            result = run_concurrent_stress(Path(support.BEAD).parent, Path(tmp) / "result.json")
        self.assertEqual(result["status"], "passed")
        self.assertEqual(result["operations"], 3000)


if __name__ == "__main__":
    unittest.main()
