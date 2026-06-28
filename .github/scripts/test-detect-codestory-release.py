#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("detect-codestory-release.py")
SPEC = importlib.util.spec_from_file_location("detect_codestory_release", SCRIPT)
assert SPEC is not None
detector = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = detector
SPEC.loader.exec_module(detector)


class AutoReleaseDecisionTest(unittest.TestCase):
    def test_retries_unchanged_unpublished_version(self) -> None:
        decision = detector.decide_release(
            old_version="0.12.5",
            new_version="0.12.5",
            tag_exists=False,
            release_exists=False,
        )

        self.assertTrue(decision.should_release)
        self.assertIn("retrying", decision.reason)

    def test_skips_unchanged_published_version(self) -> None:
        decision = detector.decide_release(
            old_version="0.12.5",
            new_version="0.12.5",
            tag_exists=True,
            release_exists=True,
        )

        self.assertFalse(decision.should_release)

    def test_releases_changed_unpublished_version(self) -> None:
        decision = detector.decide_release(
            old_version="0.12.4",
            new_version="0.12.5",
            tag_exists=False,
            release_exists=False,
        )

        self.assertTrue(decision.should_release)

    def test_refuses_changed_version_that_already_exists(self) -> None:
        with self.assertRaisesRegex(ValueError, "refusing to overwrite"):
            detector.decide_release(
                old_version="0.12.4",
                new_version="0.12.5",
                tag_exists=True,
                release_exists=True,
            )

    def test_refuses_partial_release_state(self) -> None:
        with self.assertRaisesRegex(ValueError, "partial release state"):
            detector.decide_release(
                old_version="0.12.5",
                new_version="0.12.5",
                tag_exists=True,
                release_exists=False,
            )


if __name__ == "__main__":
    unittest.main()
