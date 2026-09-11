#!/usr/bin/env python3
"""Source-cache regressions; no network, compiler, or user configuration."""
import importlib.util
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('runtime_build', Path(__file__).with_name('build.py'))
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


class SourceCacheTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        root = Path(self.temporary.name)
        original = root / 'original'
        (original / 'codex-rs/src').mkdir(parents=True)
        (original / 'codex-rs/Cargo.toml').write_text('[workspace]\n')
        (original / 'codex-rs/Cargo.lock').write_text('version = 4\n')
        (original / 'codex-rs/src/main.rs').write_text('fn main() {}\n')
        (original / 'LICENSE').write_text('fixture license\n')
        self.archive = root / 'source.tar.gz'
        with tarfile.open(self.archive, 'w:gz') as archive:
            archive.add(original, arcname='official-source')
        self.source = root / 'source-cache'

    def prepare(self):
        build.prepare_source(self.source, self.archive, [])
        self.assertTrue(build.source_complete(self.source))

    def test_complete_source_is_reused_without_extraction(self):
        self.prepare()
        with patch.object(build.tarfile, 'open', side_effect=AssertionError('unexpected extraction')):
            self.prepare()

    def test_directory_restored_without_manifest_is_reconstructed(self):
        self.prepare()
        (self.source / 'codex-rs/Cargo.toml').unlink()
        self.assertTrue(self.source.is_dir())
        self.assertFalse(build.source_complete(self.source))
        self.prepare()

    def test_other_pruned_or_modified_inputs_are_reconstructed(self):
        for relative in ['codex-rs/src/main.rs', 'codex-rs/Cargo.lock', 'LICENSE']:
            with self.subTest(relative=relative):
                self.prepare()
                expected = (self.source / relative).read_bytes()
                (self.source / relative).unlink()
                self.prepare()
                self.assertEqual((self.source / relative).read_bytes(), expected)
                (self.source / relative).write_text('modified cache input')
                self.prepare()
                self.assertEqual((self.source / relative).read_bytes(), expected)

    def test_missing_or_corrupt_inventory_is_reconstructed(self):
        self.prepare()
        (self.source / build.SOURCE_INVENTORY).unlink()
        self.prepare()
        (self.source / build.SOURCE_INVENTORY).write_text('{broken')
        self.prepare()

    def test_failed_patch_does_not_discard_existing_source(self):
        self.prepare()
        manifest = self.source / 'codex-rs/Cargo.toml'
        manifest.write_text('incomplete source retained until replacement is ready')
        with patch.object(build.subprocess, 'run', side_effect=RuntimeError('patch failed')):
            with self.assertRaisesRegex(RuntimeError, 'patch failed'):
                build.prepare_source(self.source, self.archive, ['fixture.patch'])
        self.assertEqual(manifest.read_text(), 'incomplete source retained until replacement is ready')


if __name__ == '__main__':
    unittest.main()
