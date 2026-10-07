#!/usr/bin/env python3
"""Pin-update tooling; no network, compiler, or real Codex source."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('runtime_bump', Path(__file__).with_name('bump.py'))
bump = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bump)

LOCK_BEFORE = '''version = 4

[[package]]
name = "codex-login"
version = "0.0.0"
dependencies = [
 "rustls",
]

[[package]]
name = "rustls"
version = "0.23.45"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aa"

[[package]]
name = "tokio"
version = "1.47.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "bb"
'''


def unified(path, old, new):
    return f'--- a/{path}\n+++ b/{path}\n@@ -1,3 +1,3 @@\n {old[0]}\n-{old[1]}\n+{new}\n {old[2]}\n'


class TagTests(unittest.TestCase):
    def test_stable_tags_parse_and_compare_numerically(self):
        self.assertEqual(bump.tag_version('rust-v0.160.1'), (0, 160, 1))
        self.assertLess(bump.tag_version('rust-v0.153.4'), bump.tag_version('rust-v0.160.1'))
        self.assertLess(bump.tag_version('rust-v0.99.0'), bump.tag_version('rust-v0.100.0'))

    def test_prerelease_and_malformed_tags_are_rejected(self):
        for tag in ['rust-v0.161.0-alpha.1', 'v0.160.1', 'rust-v0.160', 'rust-v0.160.1\n', 'rust-v0.160.1 `x`']:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                bump.tag_version(tag)

    def test_status_reports_behind_only_for_newer_releases(self):
        self.assertTrue(bump.status('0.153.4', 'rust-v0.160.1')['behind'])
        self.assertFalse(bump.status('0.160.1', 'rust-v0.160.1')['behind'])
        self.assertEqual(bump.status('0.153.4', 'rust-v0.160.1')['latest'], '0.160.1')

    def test_peeled_revision_prefers_the_annotated_target(self):
        output = 'c3e2\trefs/tags/rust-v0.160.1\nd277\trefs/tags/rust-v0.160.1^{}\n'
        self.assertEqual(bump.peeled_revision('rust-v0.160.1', output), 'd277')
        self.assertEqual(bump.peeled_revision('rust-v0.160.1', 'c3e2\trefs/tags/rust-v0.160.1\n'), 'c3e2')
        with self.assertRaises(ValueError):
            bump.peeled_revision('rust-v0.160.1', '')


class LockTests(unittest.TestCase):
    def test_update_lock_text_replaces_values_and_keeps_comments(self):
        text = '# pinned\nsource_version = "0.153.4"\npatches_sha256 = "old"\n'
        updated = bump.update_lock_text(text, {'source_version': '0.160.1', 'patches_sha256': 'new'})
        self.assertEqual(updated, '# pinned\nsource_version = "0.160.1"\npatches_sha256 = "new"\n')
        with self.assertRaises(ValueError):
            bump.update_lock_text(text, {'missing_key': 'x'})

    def test_registry_changes_detects_removed_or_changed_registry_packages(self):
        normalized = LOCK_BEFORE.replace('version = "0.0.0"', 'version = "0.160.1"')
        self.assertEqual(bump.registry_changes(LOCK_BEFORE, normalized), {'removed': [], 'added': []})
        bumped = normalized.replace('version = "1.47.0"', 'version = "1.48.0"')
        changes = bump.registry_changes(LOCK_BEFORE, bumped)
        self.assertEqual([p[:2] for p in changes['removed']], [('tokio', '1.47.0')])
        self.assertEqual([p[:2] for p in changes['added']], [('tokio', '1.48.0')])

    def test_stale_workspace_packages_lists_only_unnormalized_path_packages(self):
        self.assertEqual(bump.stale_workspace_packages(LOCK_BEFORE), ['codex-login'])
        self.assertEqual(bump.stale_workspace_packages(LOCK_BEFORE.replace('"0.0.0"', '"0.160.1"')), [])


class PatchTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.tree = self.root / 'tree'
        self.tree.mkdir()
        (self.tree / 'a.txt').write_text('one\ntwo\nthree\n')
        (self.tree / 'b.txt').write_text('one\nTWO\nthree\n')
        (self.tree / 'c.txt').write_text('x\ny\nz\n')
        self.patches = []
        for name, path in [('0001-clean.patch', 'a.txt'), ('0002-upstream.patch', 'b.txt'), ('0003-conflict.patch', 'c.txt')]:
            patch = self.root / name
            patch.write_text(unified(path, ['one', 'two', 'three'], 'TWO'))
            self.patches.append(patch)

    def test_apply_patches_classifies_clean_upstream_and_rejected(self):
        results = bump.apply_patches(self.tree, self.patches)
        self.assertEqual(results, {'0001-clean.patch': 'clean', '0002-upstream.patch': 'upstream', '0003-conflict.patch': ['c.txt']})
        self.assertEqual((self.tree / 'a.txt').read_text(), 'one\nTWO\nthree\n')
        self.assertTrue((self.tree / 'c.txt.rej').exists())

    def test_write_patch_set_replaces_all_patches(self):
        directory = self.root / 'patches'
        directory.mkdir()
        for patch in self.patches:
            (directory / patch.name).write_text(patch.read_text())
        written = bump.write_patch_set(directory, 'diff --git a/x b/x\n')
        self.assertEqual(sorted(p.name for p in directory.iterdir()), [bump.PATCH_NAME])
        self.assertEqual(written, [directory / bump.PATCH_NAME])
        self.assertEqual((directory / bump.PATCH_NAME).read_text(), 'diff --git a/x b/x\n')

    def test_prepare_refuses_to_discard_an_existing_rebase(self):
        target = self.root / 'rebase-0.160.1'
        (target / 'work').mkdir(parents=True)
        with self.assertRaisesRegex(SystemExit, '--force'):
            bump.ensure_fresh(target, force=False)
        self.assertTrue((target / 'work').exists())
        bump.ensure_fresh(target, force=True)
        self.assertFalse(target.exists())


class IssueTests(unittest.TestCase):
    STATUS = {'current': '0.153.4', 'latest': '0.160.1', 'tag': 'rust-v0.160.1', 'behind': True,
              'patches': {'0001-selara-writing-runtime.patch': ['codex-rs/login/Cargo.toml'], '0002-rustls-0.23.45.patch': 'upstream'}}

    def test_issue_title_names_both_versions(self):
        self.assertEqual(bump.issue_title(self.STATUS), 'Update bundled Codex runtime 0.153.4 → 0.160.1')

    def test_issue_body_explains_impact_patch_state_and_next_command(self):
        body = bump.issue_body(self.STATUS)
        self.assertIn('https://github.com/openai/codex/releases/tag/rust-v0.160.1', body)
        self.assertIn('`codex-rs/login/Cargo.toml`', body)
        self.assertIn('already upstream', body)
        self.assertIn('python3 scripts/codex-runtime/bump.py prepare 0.160.1', body)
        self.assertIn('minimal_client_version', body)

    def test_issue_rendering_rejects_unvalidated_versions(self):
        for field in ['current', 'latest', 'tag']:
            malformed = dict(self.STATUS, **{field: '0.160.1 `untrusted`'})
            for render in [bump.issue_title, bump.issue_body]:
                with self.subTest(field=field, render=render.__name__), self.assertRaises(ValueError):
                    render(malformed)

    def test_issue_command_reads_status_json(self):
        with tempfile.NamedTemporaryFile('w', suffix='.json') as status:
            json.dump(self.STATUS, status)
            status.flush()
            title = subprocess.run(['python3', str(Path(__file__).with_name('bump.py')), 'issue', status.name, '--title'],
                                   capture_output=True, text=True, check=True).stdout.strip()
        self.assertEqual(title, bump.issue_title(self.STATUS))


if __name__ == '__main__':
    unittest.main()
