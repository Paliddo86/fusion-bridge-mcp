import importlib.util
from pathlib import Path
from types import SimpleNamespace as NS
import unittest
spec=importlib.util.spec_from_file_location('hygiene',Path(__file__).parents[1]/'addin/AdomBridge/commands/document_hygiene.py')
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
def app(*docs): return NS(documents=NS(count=len(docs),item=lambda i:docs[i]))
def doc(saved,modified,name='board'): return NS(name=name,isSaved=saved,isModified=modified)
class Hygiene(unittest.TestCase):
 def test_export_does_not_clear_dirty_state(self):
  r=m.attach_document_hygiene(app(doc(False,True)),{'success':True,'_hint':'Existing hint'})
  self.assertEqual(r['documentHygiene']['needsSaveCount'],1)
  self.assertTrue(r['_hint'].startswith('Existing hint'))
  self.assertIn('separate checkpoint',r['_hint'])
 def test_previously_saved_but_modified(self):
  self.assertEqual(m.inspect_documents(app(doc(True,True)))['needsSaveCount'],1)
 def test_blank_untouched_is_not_dirty(self):
  self.assertEqual(m.inspect_documents(app(doc(False,False,'Untitled')))['needsSaveCount'],0)
 def test_unknown_is_not_reported_clean(self):
  r=m.inspect_documents(app(NS(name='unsupported')))
  self.assertFalse(r['documents'][0]['saveStateKnown'])
 def test_errors_and_original_hint_preserved(self):
  r=m.attach_document_hygiene(app(doc(False,True)),{'success':False,'error':'actual failure','_hint':'original'})
  self.assertFalse(r['success']);self.assertEqual(r['error'],'actual failure')
 def test_enumeration_failure_does_not_replace_result(self):
  r=m.attach_document_hygiene(NS(),{'success':True})
  self.assertTrue(r['success']);self.assertFalse(r['documentHygiene']['inspectionComplete'])
if __name__=='__main__':unittest.main()
