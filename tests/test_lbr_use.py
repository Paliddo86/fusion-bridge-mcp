"""lbr_use.py: a .lbr Fusion 2706 cannot ADD from (no line breaks, a byte-order mark, single-quoted
attributes: #135, #136) is rewritten keeping its element tree, and a readable one is left alone."""
import os
import tempfile
import unittest
import xml.etree.ElementTree as ET

SRC = os.path.join(os.path.dirname(__file__), '..', 'rust', 'bridge', 'src', 'lbr_use.py')
NS = {}
exec(open(SRC, encoding='utf-8').read(), NS)

COMPACT = ('<?xml version="1.0" encoding="utf-8"?>\n<!DOCTYPE eagle SYSTEM "eagle.dtd">\n<eagle version="9.6.2"><drawing><layers><layer number="1" name="Top" color="4" fill="1" visible="yes" active="yes"/></layers>'
           '<library><description>made by a &lt;b&gt;tool&lt;/b&gt;</description><packages><package name="R0603"><smd name="1" x="-0.762" y="0" dx="0.6" dy="0.7" layer="1"/>'
           '<text x="0" y="1" size="1" layer="25">&gt;NAME</text></package></packages><symbols><symbol name="R"><pin name="A" x="0" y="0"/></symbol></symbols>'
           '<devicesets><deviceset name="RES"><gates><gate name="G$1" symbol="R" x="0" y="0"/></gates><devices><device name="" package="R0603"><connects>'
           '<connect gate="G$1" pin="A" pad="1"/></connects><technologies><technology name=""/></technologies></device></devices></deviceset></devicesets></library></drawing></eagle>')
BROKEN = COMPACT.replace('><', '>\n<')
HEADER_ONLY = COMPACT[:COMPACT.index('<library>')] + BROKEN[BROKEN.index('<library>'):]


def shape(path):
    return NS['_lbr_shape'](ET.parse(path).getroot())


class LbrUse(unittest.TestCase):
    def write(self, text):
        d = tempfile.mkdtemp()
        p = os.path.join(d, 'lib.lbr')
        with open(p, 'w', encoding='utf-8') as f: f.write(text)
        return p

    def test_compact_is_rewritten_with_whitespace_only(self):
        p = self.write(COMPACT)
        before = shape(p)
        self.assertTrue(NS['normalize_lbr'](p))
        self.assertEqual(shape(p), before)
        out = open(p, encoding='utf-8').read()
        self.assertIn('<!DOCTYPE eagle SYSTEM "eagle.dtd">', out)
        self.assertIn('&gt;NAME', out)
        self.assertIn('<devicesets>\n<deviceset name="RES">', out)
        self.assertFalse(NS['lbr_needs_rewrite'](open(p, 'rb').read()))
        self.assertFalse(os.path.exists(p + '.adom-tmp'))

    def test_second_pass_and_broken_files_are_left_alone(self):
        p = self.write(COMPACT)
        NS['normalize_lbr'](p)
        once = open(p, 'rb').read()
        self.assertFalse(NS['normalize_lbr'](p))
        self.assertEqual(open(p, 'rb').read(), once)
        for text in (BROKEN, HEADER_ONLY):
            q = self.write(text)
            self.assertFalse(NS['normalize_lbr'](q))
            self.assertEqual(open(q, encoding='utf-8').read(), text)

    def test_bom_and_single_quotes_are_rewritten(self):
        for text in ('\ufeff' + BROKEN, BROKEN.replace('name="RES"', "name='RES'")):
            p = self.write(text)
            before = shape(p)
            self.assertTrue(NS['normalize_lbr'](p))
            self.assertEqual(shape(p), before)
            raw = open(p, 'rb').read()
            self.assertFalse(raw.startswith(b'\xef\xbb\xbf'))
            self.assertIn(b'name="RES"', raw)
            self.assertFalse(NS['lbr_needs_rewrite'](raw))

    def test_readable_shapes_are_left_alone(self):
        keep = ["<?xml version='1.0' encoding='utf-8'?>" + BROKEN[BROKEN.index('\n'):],   # Fusion's own exports
                BROKEN.replace('made by a', "someone's part, made by a"),                   # an apostrophe in text
                BROKEN.replace('\n', '\r\n')]
        for text in keep:
            self.assertFalse(NS['lbr_needs_rewrite'](text), text[:60])

    def test_orphan_3d_entries_are_removed_and_named(self):
        q = ('<package3d name="%s" urn="" wip_urn="urn:x:%s" type="model">\n<packageinstances>\n'
             '<packageinstance name="%s"/>\n</packageinstances>\n</package3d>\n')
        good, ghost = q % ('R0603', 'a', 'R0603'), q % ('GHOST', 'b', 'GHOST')
        text = BROKEN.replace('<symbols>', '<packages3d>\n' + good + ghost + '</packages3d>\n<symbols>', 1)
        p = self.write(text)
        self.assertTrue(NS['lbr_needs_rewrite'](open(p, 'rb').read()))
        removed = []
        self.assertTrue(NS['normalize_lbr'](p, removed))
        self.assertEqual(removed, ['GHOST'])
        out = open(p, encoding='utf-8').read()
        self.assertNotIn('GHOST', out)
        self.assertIn('name="R0603" urn=""', out)
        self.assertFalse(NS['lbr_needs_rewrite'](open(p, 'rb').read()))
        # a clean 3D entry alone is no reason to touch the file
        q2 = self.write(BROKEN.replace('<symbols>', '<packages3d>\n' + good + '</packages3d>\n<symbols>', 1))
        self.assertFalse(NS['normalize_lbr'](q2))

    def test_a_3d_entry_a_device_uses_is_kept(self):
        q = ('<package3d name="GONE" urn="" wip_urn="urn:x:used" type="model">\n<packageinstances>\n'
             '<packageinstance name="GONE"/>\n</packageinstances>\n</package3d>\n')
        text = BROKEN.replace('<symbols>', '<packages3d>\n' + q + '</packages3d>\n<symbols>', 1).replace(
            '</connects>', '</connects>\n<package3dinstances>\n<package3dinstance package3d_urn="urn:x:used"/>\n</package3dinstances>', 1)
        p = self.write(text)
        self.assertFalse(NS['normalize_lbr'](p))
        self.assertIn('GONE', open(p, encoding='utf-8').read())

    def test_a_second_path_with_the_same_name_is_refused_before_use(self):
        import sys
        sys.__dict__.pop('_adom_lbr_used', None)
        a, b = self.write(BROKEN), self.write(BROKEN)
        seen = []
        class App:
            def executeTextCommand(self, c): seen.append(c)
        NS['use_library'](App(), a)
        NS['use_library'](App(), a)   # the same path again is fine
        self.assertEqual(len(seen), 2)
        with self.assertRaises(RuntimeError) as x:
            NS['use_library'](App(), b)
        self.assertTrue(str(x.exception).startswith('library_name_in_use'))
        self.assertEqual(len(seen), 2)   # no USE sent: Fusion never gets to ask
        sys.__dict__.pop('_adom_lbr_used', None)

    def test_use_library_runs_use_after_the_rewrite(self):
        p = self.write(COMPACT)
        seen = []
        class App:
            def executeTextCommand(self, c): seen.append((c, open(p, 'rb').read()))
        self.assertTrue(NS['use_library'](App(), p))
        self.assertEqual(len(seen), 1)
        self.assertTrue(seen[0][0].startswith("Electron.run USE '"))
        self.assertFalse(NS['lbr_needs_rewrite'](seen[0][1]))


if __name__ == '__main__':
    unittest.main()
