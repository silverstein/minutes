#!/usr/bin/env python3
"""Local deterministic checks for acceptance-watcher cancellation."""

import importlib.util
import os
import pathlib
import tempfile
import unittest
from unittest import mock


SPEC = importlib.util.spec_from_file_location(
    "apple_speech_acceptance",
    pathlib.Path(__file__).with_name("test_apple_speech_signed_transport.py"),
)
ACCEPTANCE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ACCEPTANCE)


class WatcherCancellationTests(unittest.TestCase):
    def test_stop_interrupts_the_current_directory_scan(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            watcher = ACCEPTANCE.SameUidOpenHolder([root])
            for index in range(3):
                (root / f"new-{index}").write_bytes(b"synthetic watcher fixture")
            real_open = os.open

            def stop_after_first_open(*args, **kwargs):
                descriptor = real_open(*args, **kwargs)
                watcher.stop.set()
                return descriptor

            try:
                with mock.patch.object(ACCEPTANCE.os, "open", side_effect=stop_after_first_open):
                    watcher._run()
                self.assertEqual(len(watcher.held), 1)
            finally:
                for descriptor in watcher.held:
                    os.close(descriptor)

    def test_stopped_watcher_does_not_start_a_scan(self):
        watcher = ACCEPTANCE.SameUidOpenHolder([])
        watcher.stop.set()
        with mock.patch.object(ACCEPTANCE.os, "walk") as walk:
            watcher._run()
        walk.assert_not_called()


if __name__ == "__main__":
    unittest.main()
