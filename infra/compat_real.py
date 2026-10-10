#!/usr/bin/env python3
"""Real-data compatibility check (from the first independent audit): every Beads export under
~/Documents/*/.beads/issues.jsonl is imported into a fresh scratch tracker, exported again, loaded by
`br` and fed to `bv`; readiness is compared with br's. Reads the exports only; bead/br/bv run in
scratch repositories with no remotes. Writes results.json under --out. The bead it runs, and the preflight
record of it before any data is written, are infra/selected.py's: the one BEAD_UNDER_TEST names (an absolute
path), else this checkout's ./bead while it exists, else the installed one."""
import argparse, collections, json, os, pathlib, shutil, subprocess, sys, tempfile
import selected
P=pathlib.Path
parser=argparse.ArgumentParser();parser.add_argument('--out', default=tempfile.mkdtemp(prefix='bead-compat-real-'));parser.add_argument('--documents',default=os.path.expanduser('~/Documents'));args=parser.parse_args()
root=P(args.out);root.mkdir(parents=True,exist_ok=True)
bead=selected.local_bead();selected.preflight(bead)
br=shutil.which('br') or os.path.expanduser('~/.local/bin/br');bv=shutil.which('bv') or os.path.expanduser('~/.local/bin/bv')
env={k:v for k,v in os.environ.items() if not k.startswith(('BEAD_','CODEX','CLAUDE','BEADS','BV_','BD_','GIT_'))}
env.update(BEAD_HOME=str(root/'bead-home'), BEAD_AGENT='audit', BEAD_THREAD='audit-compat',XDG_CACHE_HOME=str(root/'cache'),BV_NO_CACHE='1',BV_OUTPUT_FORMAT='json')
def run(command,cwd,extra=None):
 return subprocess.run(command,cwd=cwd,env={**env,**(extra or {})},capture_output=True,text=True,timeout=60)
def parse(p):return json.loads(p.stdout)
def canon(records):
 out=[]
 for r in records:
  r=dict(r);r.pop('source_repo_path',None)
  for f in ('labels','dependencies'):
   if f in r:r[f]=sorted(r[f],key=lambda v:json.dumps(v,sort_keys=True))
  out.append(r)
 return sorted(out,key=lambda r:r['id'])
def keys(p):
 x=parse(p)
 if isinstance(x,dict):x=x.get('issues',x.get('blocked',[]))
 return sorted(i['id'] for i in x)
summary=[]
for src in sorted(P(args.documents).glob('*/.beads/issues.jsonl')):
 name=src.parent.parent.name;case=root/name;case.mkdir(exist_ok=True)
 records=[json.loads(l) for l in src.read_text().splitlines() if l.strip()]
 prefix=records[0]['id'].split('-')[0] if records else 'ex'
 r=run(['git','init','-q',str(case)],root);assert r.returncode==0,r.stderr
 extra={'BEAD_WORKSPACE':'compat-'+name}
 r=run([bead,'init','--prefix',prefix,'--import',str(src),'--json'],case,extra)
 row={'name':name,'records':len(records),'bp_import_rc':r.returncode}
 if r.returncode:row['bp_error']=r.stderr
 else:
  output=run([bead,'export'],case,extra);exported=[json.loads(l) for l in output.stdout.splitlines() if l.strip()]
  row['roundtrip_equal']=canon(records)==canon(exported)
  if not row['roundtrip_equal']:
   row['differences']=[{'id':a['id'],'fields':[k for k in a.keys()|b.keys() if a.get(k)!=b.get(k)]}for a,b in zip(canon(records),canon(exported)) if a!=b]
  (case/'bead-export.jsonl').write_text(output.stdout)
  bp_ready=run([bead,'ready','--json'],case,extra);bp_blocked=run([bead,'blocked','--json'],case,extra)
  init=run([br,'init','--prefix',prefix,'--json'],case);row['br_init_rc']=init.returncode
  (case/'.beads/issues.jsonl').write_bytes(src.read_bytes())
  imp=run([br,'sync','--import-only','--json'],case);row['br_source_import_rc']=imp.returncode
  if imp.returncode:row['br_source_error']=imp.stderr
  else:
   ready=run([br,'ready','--limit','0','--json'],case);blocked=run([br,'blocked','--limit','0','--json'],case)  # both default to 50
   row['ready_equal']=keys(ready)==keys(bp_ready);row['blocked_equal']=keys(blocked)==keys(bp_blocked)
   row['ready_counts']=[len(keys(ready)),len(keys(bp_ready))];row['blocked_counts']=[len(keys(blocked)),len(keys(bp_blocked))]
   if not row['ready_equal']:row['ready_differences']={'br_only':sorted(set(keys(ready))-set(keys(bp_ready))),'bp_only':sorted(set(keys(bp_ready))-set(keys(ready)))}
   if not row['blocked_equal']:row['blocked_differences']={'br_only':sorted(set(keys(blocked))-set(keys(bp_blocked))),'bp_only':sorted(set(keys(bp_blocked))-set(keys(blocked)))}
  if imp.returncode:
   (case/'.beads/issues.jsonl').write_text(output.stdout)
   eximp=run([br,'sync','--import-only','--json'],case);row['br_bp_export_import_rc']=eximp.returncode;row['br_bp_export_error']=eximp.stderr
  (case/'.beads/issues.jsonl').write_text(output.stdout)
  row['bv']={}
  for mode in ('triage','plan','insights'):
   p=run([bv,'--robot-'+mode,'--no-cache'],case)
   try: json.loads(p.stdout);valid=True
   except ValueError:valid=False
   row['bv'][mode]={'rc':p.returncode,'valid_json':valid}
   if p.returncode:row['bv'][mode]['error']=p.stderr
 summary.append(row);(root/'results.json').write_text(json.dumps(summary,indent=2));print(json.dumps(row),flush=True)
print(json.dumps({'trackers':len(summary),'records':sum(r['records']for r in summary),'roundtrip_equal':sum(r.get('roundtrip_equal',False)for r in summary),'br_loadable':sum(r.get('br_source_import_rc')==0 for r in summary)}),flush=True)
