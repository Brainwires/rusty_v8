#!/usr/bin/env python3
# Copyright 2018-2026 the Deno authors. MIT license.
"""Apply the denext line's patches to the `v8` and `build` submodules.

The denext line (DENEXT.md) pins both submodules at Chromium's own commits:
`v8` at a `branch-heads/<ver>` release of chromium.googlesource.com/v8/v8 and
`build` at the revision V8's DEPS names. The embedder patches upstream rusty_v8
carries as commits of denoland/v8 (`<ver>-lkgr-denoland`) and
denoland/chromium_build live here instead, as `git format-patch` files under
denext/patches/<submodule>/, applied to the checked-out submodule before a
from-source build:

  python3 tools/denext_patches.py          # apply (idempotent)
  python3 tools/denext_patches.py --check  # exit 1 unless all are applied

Each submodule's series is applied in order to a clean checkout of the pinned
commit, and `<submodule>/.denext-patches` records which series (a SHA-256 over
the patch files) is applied, so a second run is a no-op. A checkout with local
changes, or with a different series applied, is an error: reset it with
`git submodule update --force <submodule>` (and delete the stamp) first.
"""

import argparse
import hashlib
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PATCHES = os.path.join(ROOT, "denext", "patches")
SUBMODULES = ("v8", "build")


STAMP = ".denext-patches"


def git(cwd, *argv):
  return subprocess.run(
    ["git", *argv],
    cwd=cwd,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
  )


def series_id(patches):
  h = hashlib.sha256()
  for patch in patches:
    h.update(os.path.basename(patch).encode() + b"\0")
    with open(patch, "rb") as f:
      h.update(f.read())
  return h.hexdigest()


def main():
  parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
  parser.add_argument(
    "--check",
    action="store_true",
    help="only verify that every patch is applied",
  )
  args = parser.parse_args()

  missing = 0
  for sub in SUBMODULES:
    pdir = os.path.join(PATCHES, sub)
    cwd = os.path.join(ROOT, sub)
    if not os.path.isdir(pdir):
      continue
    if not os.path.exists(os.path.join(cwd, ".git")):
      sys.exit(f"error: submodule {sub} is not checked out")
    patches = [
      os.path.join(pdir, n)
      for n in sorted(os.listdir(pdir))
      if n.endswith(".patch")
    ]
    if not patches:
      continue
    # The series is applied in order (later patches touch lines of earlier
    # ones), and a stamp in the submodule records which series is applied.
    want = series_id(patches)
    stamp = os.path.join(cwd, STAMP)
    have = open(stamp).read().strip() if os.path.exists(stamp) else None
    if have == want:
      print(f"{sub}: {len(patches)} patches applied")
      continue
    if args.check:
      print(f"{sub}: patches NOT applied", file=sys.stderr)
      missing += 1
      continue
    dirty = git(cwd, "status", "--porcelain", "--untracked-files=no").stdout
    if have is not None or dirty.strip():
      sys.exit(
        f"error: {sub} has local changes or another patch series applied; "
        f"reset it to the pinned commit (git submodule update --force {sub})"
      )
    for patch in patches:
      done = git(cwd, "apply", "--whitespace=nowarn", patch)
      if done.returncode != 0:
        sys.exit(
          f"error: {sub}: {os.path.basename(patch)} does not apply:\n"
          f"{done.stderr}"
        )
      print(f"{sub}: applied {os.path.basename(patch)}")
    with open(stamp, "w") as f:
      f.write(want + "\n")
  if missing:
    sys.exit(1)


if __name__ == "__main__":
  main()
