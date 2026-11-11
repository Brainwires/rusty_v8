#!/usr/bin/env python3
# Copyright 2018-2026 the Deno authors. MIT license.
"""Tests for denext_patch_bump.py's pure parts: reading the V8 version, the
crate and tag bumps, the file edits, gitiles' log and the 14-day lag.

Usage: python3 -I tools/denext_patch_bump_test.py
"""

import datetime
import importlib.util
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location(
  "denext_patch_bump", os.path.join(HERE, "denext_patch_bump.py"))
bump = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bump)

HEADER = """\
#define V8_MAJOR_VERSION 15
#define V8_MINOR_VERSION 6
#define V8_BUILD_NUMBER 75
#define V8_PATCH_LEVEL 38
"""

LOG = """)]}'
{"log": [
  {"commit": "96c8e217cfc735cd086f00d273d692089d96cf22",
   "message": "Version 15.6.75.38\\n",
   "committer": {"time": "Tue Nov 03 12:37:55 2026"}},
  {"commit": "0ac55dd3761111111111111111111111111111111",
   "message": "[M156] [scopes] Restore skip bit\\n\\nBug: 572988453, chromium:573962535\\n",
   "committer": {"time": "Mon Nov 02 09:00:00 2026 +0100"}},
  {"commit": "32bb3bf0bb2222222222222222222222222222222",
   "message": "[M156] [modules] Do not cache star exports\\n\\nFixed: 574038247\\n",
   "committer": {"time": "Thu Oct 22 10:00:00 2026"}}
]}
"""

UTC = datetime.timezone.utc


class Versions(unittest.TestCase):
  def test_v8_version(self):
    self.assertEqual(bump.v8_version(HEADER), "15.6.75.38")
    self.assertIsNone(bump.v8_version("#define V8_MAJOR_VERSION 15\n"))

  def test_branch_of(self):
    self.assertEqual(bump.branch_of("15.6.75.38"), "15.6")

  def test_crate_bump_and_tag(self):
    self.assertEqual(bump.bump_crate_version("156.0.0+denext.4"),
                     "156.0.0+denext.5")
    self.assertEqual(bump.bump_crate_version("156.0.0+denext.9"),
                     "156.0.0+denext.10")
    self.assertEqual(bump.release_tag("156.0.0+denext.5"), "v156.0.0-denext.5")
    with self.assertRaises(ValueError):
      bump.bump_crate_version("156.0.0")


class Edits(unittest.TestCase):
  def test_cargo_files(self):
    toml = '[package]\nname = "v8"\nversion = "156.0.0+denext.4"\n'
    self.assertIn('version = "156.0.0+denext.5"',
                  bump.bump_cargo_toml(toml, "156.0.0+denext.4",
                                       "156.0.0+denext.5"))
    lock = ('[[package]]\nname = "v7"\nversion = "156.0.0+denext.4"\n\n'
            '[[package]]\nname = "v8"\nversion = "156.0.0+denext.4"\n')
    out = bump.bump_cargo_lock(lock, "156.0.0+denext.4", "156.0.0+denext.5")
    # only the v8 package moves
    self.assertIn('name = "v7"\nversion = "156.0.0+denext.4"', out)
    self.assertIn('name = "v8"\nversion = "156.0.0+denext.5"', out)

  def test_readme_and_workflow(self):
    readme = "# R\n\nV8 Version: 15.6.75.33 (Brainwires security line)\n"
    self.assertIn("V8 Version: 15.6.75.38 (Brainwires",
                  bump.bump_readme(readme, "15.6.75.38"))
    wf = "      tag:\n        default: v156.0.0-denext.4\n"
    self.assertEqual(
      bump.bump_release_workflow(wf, "v156.0.0-denext.4", "v156.0.0-denext.5"),
      "      tag:\n        default: v156.0.0-denext.5\n")
    with self.assertRaises(ValueError):
      bump.bump_release_workflow(wf, "v1.0.0-denext.1", "v1.0.0-denext.2")

  def test_denext_section_goes_first(self):
    text = "# Title\n\nIntro.\n\n## `v156.0.0-denext.4`: V8 x\n\nold\n"
    out = bump.insert_denext_section(text, "## `v156.0.0-denext.5`: V8 y\n\n")
    self.assertLess(out.index("denext.5"), out.index("denext.4"))
    self.assertTrue(out.startswith("# Title\n\nIntro.\n\n## `v156.0.0-denext.5`"))


class Log(unittest.TestCase):
  def test_merges_oldest_first_without_version_commits(self):
    merges = bump.merges(bump.gitiles_json(LOG))
    self.assertEqual([m[0] for m in merges], ["32bb3bf0bb", "0ac55dd376"])
    self.assertEqual(merges[0][2], ["574038247"])
    self.assertEqual(merges[1][2], ["572988453", "573962535"])
    # an offset is taken into account
    self.assertEqual(merges[1][3],
                     datetime.datetime(2026, 11, 2, 8, 0, tzinfo=UTC))

  def test_lag_days(self):
    oldest = datetime.datetime(2026, 10, 22, 10, 0, tzinfo=UTC)
    self.assertEqual(
      bump.lag_days(oldest, datetime.datetime(2026, 11, 5, 9, 0, tzinfo=UTC)),
      13)
    self.assertEqual(
      bump.lag_days(oldest, datetime.datetime(2026, 11, 6, 11, 0, tzinfo=UTC)),
      15)

  def test_pr_body_reports_the_gate(self):
    merges = bump.merges(bump.gitiles_json(LOG))
    body = bump.pr_body("v156.0.0-denext.5", "15.6.75.33", "15.6.75.38", 15,
                        merges, [], "denext/release-notes/v156.0.0-denext.5.md")
    self.assertIn("for 15 days, past the deno fork's 14-day", body)
    self.assertIn("`32bb3bf0bb` [M156] [modules] Do not cache star exports "
                  "(crbug 574038247).", body)
    self.assertNotIn("Public headers changed", body)
    body = bump.pr_body("v156.0.0-denext.5", "15.6.75.33", "15.6.75.38", 3,
                        merges, ["include/v8-isolate.h"], "notes.md")
    self.assertIn("within the deno fork's 14-day", body)
    self.assertIn("**Public headers changed:** `include/v8-isolate.h`", body)

  def test_release_notes(self):
    merges = bump.merges(bump.gitiles_json(LOG))
    notes = bump.release_notes("15.6.75.33", "15.6.75.38", "156.0.0+denext.5",
                               "15.6", "2026-11-03", merges, [])
    self.assertTrue(notes.startswith(
      "V8 moves from 15.6.75.33 to **15.6.75.38**, the head of "
      "`branch-heads/15.6` on 2026-11-03"))
    self.assertIn("It adds 2 merges and changes no public API", notes)


if __name__ == "__main__":
  unittest.main()
