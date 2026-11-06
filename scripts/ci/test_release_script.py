"""Publishing verified artifacts must neither rebuild them nor accept missing files."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

RELEASE_SCRIPT = Path(__file__).resolve().parents[1] / 'release.sh'


class PublishOnlyTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='bespoke-publish-test-')
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / 'scripts').mkdir()
        shutil.copy2(RELEASE_SCRIPT, self.root / 'scripts/release.sh')
        (self.root / 'Cargo.toml').write_text('[package]\nname = "fixture"\nversion = "1.0.0"\n')
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.tool('git', '''case "$1" in
status) exit 0 ;;
branch) echo origin/main ;;
rev-parse) echo 1111111111111111111111111111111111111111 ;;
*) exit 2 ;;
esac
''')
        self.tool('gh', '''printf '%s\\n' "$*" >> "$BESPOKE_TEST_GH_LOG"
case "$1 $2" in
'release view') exit 1 ;;
'release create') exit 0 ;;
*) exit 2 ;;
esac
''')
        for command in ('cargo', 'curl', 'xcrun', 'codesign', 'ditto'):
            self.tool(command, 'echo "Unexpected rebuild or packaging command" >&2\nexit 99\n')
        self.log = self.root / 'gh.log'
        self.env = dict(os.environ, PATH=str(self.bin) + os.pathsep + os.environ['PATH'],
                        BESPOKE_TEST_GH_LOG=str(self.log))

    def tool(self, name, body):
        path = self.bin / name
        path.write_text('#!/bin/sh\n' + body)
        path.chmod(0o755)

    def publish(self):
        return subprocess.run(['bash', str(self.root / 'scripts/release.sh'), '--publish-only'],
                              env=self.env, text=True, capture_output=True)

    def test_missing_artifacts_are_not_published(self):
        result = self.publish()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Build and verify', result.stderr)
        self.assertNotIn('release create', self.log.read_text())

    def test_existing_artifacts_are_published_without_rebuilding(self):
        dist = self.root / 'dist/1.0.0'
        dist.mkdir(parents=True)
        archive, appcast = dist / 'Bespoke-1.0.0.zip', dist / 'appcast.xml'
        archive.write_bytes(b'verified archive fixture')
        appcast.write_text('<rss>verified appcast fixture</rss>')
        result = self.publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('release create v1.0.0', self.log.read_text())
        self.assertEqual(archive.read_bytes(), b'verified archive fixture')
        self.assertEqual(appcast.read_text(), '<rss>verified appcast fixture</rss>')


if __name__ == '__main__':
    unittest.main()
