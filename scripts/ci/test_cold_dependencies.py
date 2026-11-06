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
    def test_previous_attempt_is_rejected_before_extraction(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'ui.json').write_text(json.dumps({'run': '123', 'attempt': '1'}))
            with patch.object(cold, 'identity', return_value={'run': '123', 'attempt': '2'}), \
                    patch.object(cold.subprocess, 'run') as run:
                with self.assertRaisesRegex(SystemExit, 'run attempt'):
                    cold.restore(root)
                run.assert_not_called()

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
            subprocess.run(command, cwd=root, env=env, check=True, capture_output=True)
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
