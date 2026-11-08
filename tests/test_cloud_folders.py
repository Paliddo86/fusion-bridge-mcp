"""fusion_save_to_cloud makes the folders on folderPath that are missing (its description promises it);
every other cloud verb only navigates. Found live 2026-11-05: a mechanical save to fusion-lane-tests/drawings
answered "Folder 'drawings' not found"."""
import importlib
from pathlib import Path
import sys
from types import SimpleNamespace as NS
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import test_mechanical  # noqa: F401  (installs the adsk stand-ins and the commands package)

cd = importlib.import_module('mechanical_test_commands.cloud_documents')


class Folders:
    def __init__(self, owner, names=()):
        self.owner, self.values, self.added = owner, [Folder(n) for n in names], []

    @property
    def count(self):
        return len(self.values)

    def item(self, i):
        return self.values[i]

    def add(self, name):
        made = Folder(name)
        self.values.append(made)
        self.added.append(name)
        return made


class Folder:
    def __init__(self, name, children=()):
        self.name = name
        self.dataFolders = Folders(self, children)


class ResolveFolder(unittest.TestCase):
    def setUp(self):
        self.root = Folder("Default Project")
        self.tests = Folder("fusion-lane-tests")
        self.root.dataFolders.values.append(self.tests)
        self.project = NS(rootFolder=self.root, name="Default Project")

    def test_navigation_alone_never_creates(self):
        folder, err = cd._resolve_folder(self.project, "fusion-lane-tests/drawings")
        self.assertIsNone(folder)
        self.assertIn("'drawings' not found", err)
        self.assertEqual(self.tests.dataFolders.added, [])

    def test_create_makes_each_missing_folder_once(self):
        folder, err = cd._resolve_folder(self.project, "Fusion-Lane-Tests\\drawings/2026", create=True)
        self.assertIsNone(err)
        self.assertEqual(folder.name, "2026")
        # the existing folder is matched by name in any case; only the missing ones are made
        self.assertEqual((self.root.dataFolders.added, self.tests.dataFolders.added), ([], ["drawings"]))
        again, _ = cd._resolve_folder(self.project, "fusion-lane-tests/drawings/2026", create=True)
        self.assertIs(again, folder)
        self.assertEqual(self.tests.dataFolders.added, ["drawings"])

    def test_a_folder_fusion_will_not_make_is_reported(self):
        self.tests.dataFolders.add = lambda name: None
        folder, err = cd._resolve_folder(self.project, "fusion-lane-tests/drawings", create=True)
        self.assertIsNone(folder)
        self.assertIn("Could not create folder 'drawings'", err)


if __name__ == "__main__":
    unittest.main()
