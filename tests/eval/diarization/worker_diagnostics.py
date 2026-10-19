"""Closed diagnostic receipt fields for the supervised offline worker."""
import re

PHASES = frozenset(('handles_started', 'handles_closed', 'awaiting_pcm',
                    'pcm_loaded', 'pipeline_ready', 'inference_complete'))
FAILURES = frozenset(('model_load', 'inference', 'unsupported_mode', 'segmentation',
                      'powerset', 'plda', 'reconstruct', 'queue', 'invariant',
                      'worker_panic', 'worker_spawn', 'backend', 'other', 'unclassified'))


def parse_worker_diagnostics(text):
    def last_label(field, allowed):
        values = re.findall(r'\b' + field + r'="([a-z_]+)"', text)
        return next((value for value in reversed(values) if value in allowed), 'unreported')

    values = re.findall(r'\bworker_exit_code=Some\((-?\d{1,11})\)', text)
    code = int(values[-1]) if values else None
    if code is not None and not -2**31 <= code < 2**31:
        code = None
    return dict(last_worker_phase=last_label('worker_phase', PHASES),
                worker_failure_class=last_label('worker_failure', FAILURES),
                worker_exit_code=code)
