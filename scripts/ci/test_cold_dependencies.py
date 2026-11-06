"""Check artifact identity and Cargo freshness across dependency transfers."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('cold_dependencies', Path(__file__).with_name('cold-dependencies.py'))
cold = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cold)


class DependencyTransferTests(unittest.TestCase):
    def test_ui_worker_omits_libraries_and_shared_prerequisites(self):
        edges = {
            'app': ['gpui', 'gpui-component', 'scrawl', 'cpal', 'arboard'],
            'gpui': ['shared', 'ui-only', 'arboard'],
            'gpui-component': ['gpui', 'widgets-only'],
            'scrawl': ['gpui', 'native', 'shared'],
            'cpal': ['shared'], 'arboard': ['shared'],
            'shared': [], 'ui-only': [], 'widgets-only': [], 'native': [],
        }
        metadata = {
            'packages': [dict(id=name, name=name) for name in edges],
            'resolve': {
                'root': 'app',
                'nodes': [dict(id=name, deps=[dict(pkg=dep, dep_kinds=[dict(kind=None)]) for dep in deps])
                          for name, deps in edges.items()],
            },
        }
        groups = cold.dependency_groups(metadata)
        self.assertEqual(groups['ui'], {'ui-only', 'widgets-only'})
        self.assertEqual(groups['backend'], {'cpal'})
        self.assertEqual(groups['platform'], {'arboard', 'native', 'shared'})
        self.assertFalse({'app', 'gpui', 'gpui-component', 'scrawl'} & set.union(*groups.values()))

    def test_previous_attempt_is_rejected_before_extraction(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'ui.json').write_text(json.dumps({'run': '123', 'attempt': '1'}))
            with patch.object(cold, 'identity', return_value={'run': '123', 'attempt': '2'}), \
                    patch.object(cold.subprocess, 'run') as run:
                with self.assertRaisesRegex(SystemExit, 'run attempt'):
                    cold.restore(root)
                run.assert_not_called()

    def test_single_ui_transfer_does_not_wait_for_other_groups(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.dict(os.environ, GITHUB_REPOSITORY='owner/repo', GITHUB_RUN_ID='123', GITHUB_RUN_ATTEMPT='2'), \
                patch.object(cold, 'output', return_value=json.dumps([
                    {'artifacts': [{'name': 'cold-deps-2-ui', 'expired': False}]}])) as output, \
                patch.object(cold.subprocess, 'run') as run, patch.object(cold.time, 'sleep') as sleep:
            cold.download(Path(directory), ['ui'])
            output.assert_called_once()
            run.assert_called_once()
            self.assertIn('cold-deps-2-ui', run.call_args.args[0])
            sleep.assert_not_called()

    def test_failed_dependency_job_stops_download_wait(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.dict(os.environ, GITHUB_REPOSITORY='owner/repo', GITHUB_RUN_ID='123', GITHUB_RUN_ATTEMPT='2'), \
                patch.object(cold, 'output', side_effect=[
                    json.dumps([{'artifacts': [{'name': 'cold-deps-1-ui', 'expired': False}]}]),
                    json.dumps({'jobs': [{'name': 'Dependencies ui', 'status': 'completed', 'conclusion': 'failure'}]})]), \
                patch.object(cold.subprocess, 'run') as run, patch.object(cold.time, 'sleep') as sleep:
            with self.assertRaisesRegex(SystemExit, 'Dependency jobs failed'):
                cold.download(Path(directory))
            run.assert_not_called()
            sleep.assert_not_called()

    @unittest.skipUnless(shutil.which('cargo'), 'Cargo is required for the freshness regression')
    def test_merged_output_times_do_not_force_recompilation(self):
        with tempfile.TemporaryDirectory(prefix='bespoke-fingerprint-') as directory:
            root = Path(directory)
            (root / 'dep/src').mkdir(parents=True)
            (root / 'src').mkdir()
            (root / 'Cargo.toml').write_text('[package]\nname="consumer"\nversion="0.1.0"\nedition="2021"\n'
                                           '[dependencies]\nfixturedep={path="dep"}\n')
            (root / 'dep/Cargo.toml').write_text('[package]\nname="fixturedep"\nversion="0.1.0"\nedition="2021"\n')
            (root / 'dep/src/lib.rs').write_text('pub fn value() -> u8 { 42 }\n')
            (root / 'src/lib.rs').write_text('pub fn read() -> u8 { fixturedep::value() }\n')
            epoch = int(time.time()) - 100
            cold.set_times(root, epoch)
            env = dict(os.environ, CARGO_TARGET_DIR=str(root / 'target'))
            command = ['cargo', 'build', '--offline', '--release', '-v']
            subprocess.run(['cargo', 'build', '--offline', '--release', '-p', 'fixturedep'],
                           cwd=root, env=env, check=True, capture_output=True)
            # The primary-package copy is redundant when another crate consumes it.
            (root / 'target/release/libfixturedep.rlib').unlink()
            first = subprocess.run(command, cwd=root, env=env, text=True, check=True, capture_output=True)
            self.assertIn('Fresh fixturedep', first.stderr)
            # Simulate restoring an identical dependency built by a later worker.
            for artifact in (root / 'target/release/deps').glob('libfixturedep-*'):
                os.utime(artifact, (time.time() + 5, time.time() + 5))
            stale = subprocess.run(command, cwd=root, env=env, text=True, capture_output=True, check=True)
            self.assertIn('Dirty consumer', stale.stderr)
            cold.set_times(root / 'target/release', epoch + 1)
            fresh = subprocess.run(command, cwd=root, env=env, text=True, capture_output=True, check=True)
            self.assertIn('Fresh consumer', fresh.stderr)
            self.assertNotIn('Dirty', fresh.stderr)


if __name__ == '__main__':
    unittest.main()
