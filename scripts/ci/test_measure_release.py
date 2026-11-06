"""Report observed time unless a limit is requested, and reject failed builds."""
import contextlib
import datetime
import io
import json
from pathlib import Path
import runpy
import unittest
from unittest.mock import patch

REPORTER = Path(__file__).with_name('measure-release.py')


def stamp(seconds):
    return (datetime.datetime(2026, 1, 1, tzinfo=datetime.timezone.utc)
            + datetime.timedelta(seconds=seconds)).isoformat().replace('+00:00', 'Z')


def job(name, start, end, conclusion='success', steps=True):
    return dict(name=name, startedAt=stamp(start), completedAt=stamp(end),
                status='completed', conclusion=conclusion,
                steps=[dict(name='Build', startedAt=stamp(start), completedAt=stamp(end),
                            conclusion=conclusion)] if steps else [])


class ReleaseBudgetTests(unittest.TestCase):
    def report(self, jobs, extra_args=()):
        data = json.dumps(dict(url='https://example.test/run', jobs=jobs))
        output = io.StringIO()
        with patch('subprocess.check_output', return_value=data), \
                patch('sys.argv', ['measure-release.py', '123', *extra_args]), \
                contextlib.redirect_stdout(output), self.assertRaises(SystemExit) as caught:
            runpy.run_path(str(REPORTER), run_name='__main__')
        return caught.exception.code, output.getvalue()

    def test_time_is_informational_by_default(self):
        code, text = self.report([job('release', 0, 671)])
        self.assertEqual(code, 0)
        self.assertIn('11m11s, success', text)
        self.assertNotIn('FAIL', text)

    def test_explicit_limit_is_enforced(self):
        code, text = self.report([job('release', 0, 480)], ['--max-seconds', '480'])
        self.assertEqual(code, 1)
        self.assertIn('FAIL <480s', text)

    def test_explicit_limit_accepts_shorter_build(self):
        code, text = self.report([job('release', 0, 479)], ['--max-seconds', '480'])
        self.assertEqual(code, 0)
        self.assertIn('PASS <480s', text)

    def test_running_build_cannot_pass(self):
        running = job('release', 0, 100)
        running['status'] = 'in_progress'
        self.assertEqual(self.report([running])[0], 1)

    def test_unallocated_runner_has_no_build_measurement(self):
        code, text = self.report([job('release', 0, 900, 'cancelled', steps=False)])
        self.assertEqual(code, 1)
        self.assertIn('no runner steps executed', text)
        self.assertNotIn('15m00s', text)

    def test_dependencies_without_release_cannot_pass(self):
        self.assertNotEqual(self.report([job('Dependencies ui', 0, 100)])[0], 0)

    def test_failed_build_cannot_pass(self):
        self.assertEqual(self.report([job('release', 0, 100, 'failure')])[0], 1)


if __name__ == '__main__':
    unittest.main()
