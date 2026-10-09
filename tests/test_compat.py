"""Beads compatibility: import/export, comment ids, real tracker exports, ref names.

Gathered from: first independent audit (AUDIT), 2026-10-06. Docstrings and names that cite
finding ids (A.., R.., N.., C.., K.., P.., D..) refer to FINDINGS.md."""
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
class CompatAuditCommentIds(unittest.TestCase):
    def test_existing_comment_export_id_survives_clock_skewed_new_comment(self):
        bead=support.load_module();j=bead.Journal()
        def uid(ms):return str(uuid.UUID(int=(ms<<80)|(7<<76)|(2<<62)))
        create=uid(1000);first=uid(3000);later=uid(2000)
        for eid,deps,op,data in [(create,[],'create',{'key':'ex-a','fields':{'title':'a'}}),(first,[create],'comment',{'text':'first'}),(later,[first],'comment',{'text':'later clock moved back'})]:
            if eid==later:before=bead.export_records(bead.replay(j))[0]['comments'][0]['id']
            j.add({'v':1,'id':eid,'replica':'a','deps':deps,'actor':{'agent':'audit'},'op':op,'issue':create,'data':data},'a')
        after=next(c['id']for c in bead.export_records(bead.replay(j))[0]['comments']if c['text']=='first')
        self.assertEqual(after,before,'exported existing comment id changed after another comment')


# ---- from test_audit.py: first independent audit (AUDIT), 2026-10-06
class CompatAuditRealCompatibility(unittest.TestCase):
    def test_all_available_real_exports_and_br_bv_consumers(self):
        """Read only source .jsonl; helper copies into its own scratch repos with no remotes."""
        import tempfile
        import shutil
        helper=Path(support.BEAD).resolve().parent/'infra'/'compat_real.py'
        exports=sorted(Path(os.path.expanduser('~/Documents')).glob('*/.beads/issues.jsonl'))
        if not (shutil.which('br') or Path(os.path.expanduser('~/.local/bin/br')).exists()) \
                or not (shutil.which('bv') or Path(os.path.expanduser('~/.local/bin/bv')).exists()) or not exports:
            self.skipTest('br, bv or real Beads exports unavailable on this machine')
        with tempfile.TemporaryDirectory(prefix='bead-compat-real-') as tmp:
            proc=subprocess.run([os.sys.executable,str(helper),'--out',tmp],capture_output=True,text=True,timeout=180)
            self.assertEqual(proc.returncode,0,proc.stderr)
            results=json.loads((Path(tmp)/'results.json').read_text())
            self.assertGreater(len(results),0)
            for row in results:
                with self.subTest(tracker=row['name']):
                    self.assertEqual(row['bp_import_rc'],0)
                    self.assertTrue(row['roundtrip_equal'])
                    if row['br_source_import_rc']==0:
                        self.assertTrue(row['ready_equal']);self.assertTrue(row['blocked_equal'])
                    else:
                        self.assertNotEqual(row['br_bp_export_import_rc'],0)
                    for mode in ('triage','plan','insights'):
                        self.assertEqual(row['bv'][mode],{'rc':0,'valid_json':True})


# ---- from test_audit.py: first independent audit (AUDIT), 2026-10-06
class CompatAuditRefValidation(unittest.TestCase):
    def test_ref_names_reject_code_refs_and_traversal_accept_unicode(self):
        bead=support.load_module()
        import tempfile
        with tempfile.TemporaryDirectory(prefix='bead-ref-validation-') as tmp, mock.patch.dict(os.environ,{'BEAD_HOME':tmp,'BEAD_WORKSPACE':'ref-audit'}):
            for ref in ('main','refs/heads/main','refs/tags/v1','refs/remotes/origin/main','refs/beads/../foo','refs/beads/bad..name'):
                with self.subTest(ref=ref),self.assertRaises(bead.BeadError) as caught:
                    bead.Workspace('ref-audit',None,None,{'ref':ref}).ref
                self.assertEqual(caught.exception.code,'CONFIG')
            for ref in ('refs/beads/journal','refs/beads/δ','refs/tracker/journal'):
                with self.subTest(ref=ref):
                    self.assertEqual(bead.Workspace('ref-audit',None,None,{'ref':ref}).ref,ref)


if __name__ == "__main__":
    unittest.main()


# ---- after review 7: the beads_rust tracker on linux-1 could not be imported (one record's status is "invalid")
class CompatCustomStatuses(unittest.TestCase):
    """Beads tools keep a status they do not know (beads_rust: Status::Custom); so does bead's import, and such
    an issue is neither ready nor done. bead's own commands still set only bead's vocabulary."""
    def setUp(self):
        self.box=support.Sandbox(); self.addCleanup(self.box.cleanup)

    def test_imported_custom_statuses_are_kept_and_never_ready(self):
        base={"priority":2,"issue_type":"task","created_at":"2026-03-23T02:55:33Z","created_by":"someone",
              "updated_at":"2026-03-23T02:55:42Z","source_repo":".","compaction_level":0,"original_size":0}
        records=[{"id":"ex-open","title":"Open","status":"open",**base},
                 {"id":"ex-pin","title":"Pinned","status":"pinned",**base},
                 {"id":"ex-odd","title":"Test issue","status":"invalid",**{k:v for k,v in base.items() if k!="created_by"}},
                 {"id":"ex-bad","title":"Bad","status":"Not A Status","issue_type":"task","priority":2}]
        source=self.box.root/"issues.jsonl"
        source.write_text("".join(json.dumps(r)+"\n" for r in records[:3]))
        a=support.Machine(self.box,"a")
        self.assertEqual(a.bead("init","--prefix","ex","--import",str(source))["imported"],3)
        self.assertEqual({json.loads(l)["id"]:json.loads(l)["status"] for l in a.export().splitlines()},
                         {"ex-open":"open","ex-pin":"pinned","ex-odd":"invalid"})
        self.assertEqual(a.ready(),{"ex-open"})
        self.assertEqual(a.bead("blocked"),[])
        self.assertEqual({i["id"] for i in a.bead("list","--limit","0")["issues"]},{"ex-open","ex-pin","ex-odd"})
        self.assertEqual(a.show("ex-odd")["status"],"invalid")
        # a record without created_by (28 of beads_rust's 1,031) is not attributed to the importer
        self.assertNotIn("created_by",next(json.loads(l) for l in a.export().splitlines() if '"ex-odd"' in l))
        self.assertIn("by unknown",a.run("show","ex-odd").stdout)
        a.bead("update","ex-odd","--title","still custom")
        self.assertEqual(a.show("ex-odd")["status"],"invalid")
        refused=a.run("update","ex-open","--status","invalid","--json")
        self.assertEqual((refused.returncode,json.loads(refused.stderr)["error"]["code"]),(1,"VALIDATION"))
        a.bead("close","ex-odd")
        self.assertEqual(a.show("ex-odd")["status"],"closed")
        source.write_text(json.dumps(records[3])+"\n")
        malformed=a.run("import",str(source),"--json")
        self.assertEqual((malformed.returncode,json.loads(malformed.stderr)["error"]["code"]),(1,"VALIDATION"))
        self.assertIn("unknown status",malformed.stderr)
