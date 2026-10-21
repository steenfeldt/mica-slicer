#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Provenance: original
"""Specification tests for the exact comparator (Python standard library only)."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("gcode-exact-diff.py")
SPEC = importlib.util.spec_from_file_location("gcode_exact_diff", SCRIPT)
DIFF = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DIFF
SPEC.loader.exec_module(DIFF)


class ExactDiff(unittest.TestCase):
    def test_numeric_tolerances_and_word_presence(self):
        a = DIFF.Command("G1", dict(X=10., E=.1, F=1200.), 1)
        b = DIFF.Command("G1", dict(X=10.002, E=.1001, F=1201.), 1)
        self.assertFalse(DIFF.differences(a, b))
        b.words.update(X=10.0021, E=.10011, F=1201.01)
        self.assertEqual(DIFF.differences(a, b), ["E", "F", "X"])
        del b.words["E"]
        self.assertIn("E presence", DIFF.differences(a, b))

    def test_relative_extrusion_labels_and_shutdown_boundary(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input.gcode"
            path.write_text("M82\nG92 E10\n;LAYER_CHANGE\nM73 P1\nM486 S0\n"
                            "G1 X1 E10.25 F100 ; comment\n;LAYER_CHANGE\n"
                            "G1 E10.15 F200\n;TYPE:Custom\nG1 X999\n")
            layers = DIFF.read(path)
        self.assertEqual([len(layer) for layer in layers], [1, 1])
        self.assertAlmostEqual(layers[0][0].words["E"], .25)
        self.assertAlmostEqual(layers[1][0].words["E"], -.1)

    def test_normalization_is_idempotent_with_extrusion_mode_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            a, b, c = [Path(directory) / name for name in ("a", "b", "c")]
            a.write_text("M83\n;LAYER_CHANGE\nG1 E.5\nM82\nG92 E20\nG1 E20.25\n")
            for source, target in [(a, b), (b, c)]:
                subprocess.run([sys.executable, str(SCRIPT), str(source), "--normalize", str(target)], check=True)
            self.assertEqual(b.read_text(), c.read_text())

    def test_missing_and_duplicated_moves_are_not_hidden(self):
        a = DIFF.Command("G1", dict(X=1.), 1)
        self.assertEqual(DIFF.differences(a, None), ["missing"])
        self.assertEqual(DIFF.differences(a, DIFF.Command("G0", a.words, 1)), ["command"])

    def test_absent_layer_markers_are_an_error(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "empty.gcode"
            path.write_text("G1 X1\n")
            with self.assertRaises(ValueError):
                DIFF.read(path)


if __name__ == "__main__":
    unittest.main()
