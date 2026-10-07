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
        bp=support.load_module();j=bp.Journal()
        def uid(ms):return str(uuid.UUID(int=(ms<<80)|(7<<76)|(2<<62)))
        create=uid(1000);first=uid(3000);later=uid(2000)
        for eid,deps,op,data in [(create,[],'create',{'key':'ex-a','fields':{'title':'a'}}),(first,[create],'comment',{'text':'first'}),(later,[first],'comment',{'text':'later clock moved back'})]:
            if eid==later:before=bp.export_records(bp.replay(j))[0]['comments'][0]['id']
            j.add({'v':1,'id':eid,'replica':'a','deps':deps,'actor':{'agent':'audit'},'op':op,'issue':create,'data':data},'a')
        after=next(c['id']for c in bp.export_records(bp.replay(j))[0]['comments']if c['text']=='first')
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
        with tempfile.TemporaryDirectory(prefix='bp-compat-real-') as tmp:
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
        bp=support.load_module()
        import tempfile
        with tempfile.TemporaryDirectory(prefix='bp-ref-validation-') as tmp, mock.patch.dict(os.environ,{'BEAD_HOME':tmp,'BEAD_WORKSPACE':'ref-audit'}):
            for ref in ('main','refs/heads/main','refs/tags/v1','refs/remotes/origin/main','refs/beads/../foo','refs/beads/bad..name'):
                with self.subTest(ref=ref),self.assertRaises(bp.BpError) as caught:
                    bp.Workspace('ref-audit',None,None,{'ref':ref}).ref
                self.assertEqual(caught.exception.code,'CONFIG')
            for ref in ('refs/beads/journal','refs/beads/δ','refs/tracker/journal'):
                with self.subTest(ref=ref):
                    self.assertEqual(bp.Workspace('ref-audit',None,None,{'ref':ref}).ref,ref)


if __name__ == "__main__":
    unittest.main()
