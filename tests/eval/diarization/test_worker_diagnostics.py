import unittest
from worker_diagnostics import parse_worker_diagnostics


class WorkerDiagnosticTests(unittest.TestCase):
    def test_closed_labels_and_native_exit_status_survive_without_payload(self):
        receipt = parse_worker_diagnostics(
            'private/path or audio\nworker_phase="pcm_loaded" worker_phase="pipeline_ready" '
            'worker_failure="backend" private context worker_exit_code=Some(-1073741819)')
        self.assertEqual(receipt, dict(last_worker_phase='pipeline_ready',
                                      worker_failure_class='backend',
                                      worker_exit_code=-1073741819))
        self.assertNotIn('private', str(receipt))

    def test_unknown_labels_and_unbounded_status_are_not_published(self):
        for code in ('Some(2147483648)', 'Some(-2147483649)', 'Some(123456789012)',
                     'Some(private)', 'None'):
            receipt = parse_worker_diagnostics(
                'worker_phase="private_path" worker_failure="private_audio" '
                'worker_exit_code=' + code)
            self.assertEqual(receipt, dict(last_worker_phase='unreported',
                                          worker_failure_class='unreported',
                                          worker_exit_code=None))


if __name__ == '__main__':
    unittest.main()
