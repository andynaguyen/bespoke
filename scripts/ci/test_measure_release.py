"""The budget must include dependency jobs and reject missing build results."""
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
    def report(self, jobs):
        data = json.dumps(dict(url='https://example.test/run', jobs=jobs))
        output = io.StringIO()
        with patch('subprocess.check_output', return_value=data), \
                patch('sys.argv', ['measure-release.py', '123']), \
                contextlib.redirect_stdout(output), self.assertRaises(SystemExit) as caught:
            runpy.run_path(str(REPORTER), run_name='__main__')
        return caught.exception.code, output.getvalue()

    def test_individually_fast_jobs_can_exceed_pipeline_budget(self):
        code, text = self.report([job('Dependencies ui', 0, 300), job('measure / release', 300, 600)])
        self.assertEqual(code, 1)
        self.assertIn('Pipeline elapsed: 10m00s, FAIL', text)

    def test_parallel_jobs_count_wall_time_not_sum(self):
        code, text = self.report([job('Dependencies ui', 0, 200), job('Dependencies platform', 0, 180),
                                  job('Dependencies backend', 0, 150), job('measure / release', 200, 400)])
        self.assertEqual(code, 0)
        self.assertIn('Pipeline elapsed: 6m40s, PASS', text)

    def test_exactly_eight_minutes_fails(self):
        self.assertEqual(self.report([job('release', 0, 480)])[0], 1)

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
