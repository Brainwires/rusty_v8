#!/usr/bin/env python3
# Copyright 2018-2026 the Deno authors. MIT license.
"""Moves the denext line's `v8` submodule to the head of its V8 branch.

The denext line (DENEXT.md) pins `v8` at a release on Chromium's
`branch-heads/<major>.<minor>` and takes Google's security merges there as
patch-level bumps. This is upstream's tools/auto_update_v8.ts, pointed at the
branch the pin is on instead of a denoland tracking branch, and run by hand
(.github/workflows/patch-bump-denext.yml, workflow_dispatch only):

1. reads the pinned V8 version from v8/include/v8-version.h and asks
   chromium.googlesource.com for the head of its branch;
2. stops when the pin is the head; otherwise fetches the head and refuses a
   head whose DEPS moved (a bump that needs tools/update_deps.py and a look
   at the other submodules is not a patch bump) or that the
   denext/patches/v8 series no longer applies to;
3. bumps the crate's `+denext.<n>` build metadata (Cargo.toml, Cargo.lock),
   the release workflow's default tag and the README's V8 version, and writes
   the release notes and a DENEXT.md section listing the merges;
4. commits on `denext/v8-<version>`, pushes, and opens (or retitles) a pull
   request against the base branch. The release follows the merge: dispatch
   release-denext.yml with the new tag.

It also reports how long the pin has lacked the oldest merge it is missing
(the deno fork's engine staleness gate fails a runtime tag once that passes
PATCH_LAG_DAYS days), in the log and the pull request.

Usage: denext_patch_bump.py [--base <branch>] [--dry-run] [--now <ISO time>]
Env:   GH_TOKEN (the push and `gh pr`, not needed with --dry-run)
Exits 0 (bumped, or nothing to do), or 1 with the reason.
"""

import argparse
import base64
import datetime
import glob
import json
import os
import re
import subprocess
import sys
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
V8_REMOTE = "https://chromium.googlesource.com/v8/v8"
# A file at a revision, base64-encoded.
V8_HEADER_URL = V8_REMOTE + "/+/{rev}/include/v8-version.h?format=TEXT"
# How much of the branch to fetch: each merge is two commits (the fix and its
# `Version` commit), so this is months of a release branch.
BRANCH_DEPTH = 500
# Gitiles' log of a range, newest first.
V8_LOG_URL = V8_REMOTE + "/+log/{old}..{new}?format=JSON&n=500"
# The deno fork's engine staleness gate allows a pin this many days behind.
PATCH_LAG_DAYS = 14
TIMEOUT_SECONDS = 30
RELEASE_WORKFLOW = ".github/workflows/release-denext.yml"


def v8_version(header):
  """"15.6.75.38" from a v8-version.h, or None."""
  parts = []
  for macro in ("MAJOR_VERSION", "MINOR_VERSION", "BUILD_NUMBER",
                "PATCH_LEVEL"):
    m = re.search(rf"^#define V8_{macro} (\d+)\s*$", header, re.M)
    if not m:
      return None
    parts.append(m.group(1))
  return ".".join(parts)


def branch_of(version):
  """The `branch-heads/` name of a V8 version: 15.6.75.38 -> 15.6."""
  return ".".join(version.split(".")[:2])


def bump_crate_version(version):
  """156.0.0+denext.4 -> 156.0.0+denext.5."""
  m = re.match(r"^(\d+\.\d+\.\d+)\+denext\.(\d+)$", version)
  if not m:
    raise ValueError(f"not a denext crate version: {version!r}")
  return f"{m.group(1)}+denext.{int(m.group(2)) + 1}"


def release_tag(crate_version):
  """156.0.0+denext.5 -> v156.0.0-denext.5 (release-denext.yml's form)."""
  return "v" + crate_version.replace("+", "-", 1)


def crate_version(cargo_toml):
  m = re.search(r'^version = "([^"]+)"$', cargo_toml, re.M)
  if not m:
    raise ValueError("no version in Cargo.toml")
  return m.group(1)


def replace_once(text, pattern, replacement, what):
  new, n = re.subn(pattern, replacement, text, count=1, flags=re.M)
  if n != 1:
    raise ValueError(f"could not find {what}")
  return new


def bump_cargo_toml(text, old, new):
  return replace_once(text, rf'^version = "{re.escape(old)}"$',
                      f'version = "{new}"', "the crate version in Cargo.toml")


def bump_cargo_lock(text, old, new):
  return replace_once(
    text, rf'^(name = "v8"\nversion = )"{re.escape(old)}"$', rf'\1"{new}"',
    "the v8 package in Cargo.lock")


def bump_readme(text, version):
  return replace_once(text, r"^V8 Version: \S+", f"V8 Version: {version}",
                      "the V8 version line in README.md")


def bump_release_workflow(text, old_tag, new_tag):
  return replace_once(text, rf"^(\s+default: ){re.escape(old_tag)}$",
                      rf"\g<1>{new_tag}", f"default: {old_tag} in "
                      f"{RELEASE_WORKFLOW}")


def gitiles_json(text):
  """A gitiles JSON answer, without its `)]}'` XSSI guard line."""
  if text.startswith(")]}'"):
    text = text.split("\n", 1)[1] if "\n" in text else ""
  return json.loads(text)


def gitiles_time(value):
  """A gitiles commit time ("Tue Nov 03 12:37:55 2026", sometimes with a
  " +0000" offset) as an aware UTC datetime."""
  m = re.match(
    r"^(\w{3} \w{3} +\d{1,2} \d{2}:\d{2}:\d{2} \d{4})(?: ([+-]\d{4}))?$",
    value.strip())
  if not m:
    raise ValueError(f"unreadable gitiles time {value!r}")
  t = datetime.datetime.strptime(re.sub(" +", " ", m.group(1)),
                                 "%a %b %d %H:%M:%S %Y")
  offset = m.group(2) or "+0000"
  sign = 1 if offset[0] == "+" else -1
  delta = datetime.timedelta(hours=int(offset[1:3]), minutes=int(offset[3:5]))
  return (t - sign * delta).replace(tzinfo=datetime.timezone.utc)


def merges(log_json):
  """The commits of a gitiles range log, oldest first, as (short sha,
  subject, bug ids, datetime); a `Version x.y.z.w` commit is left out (it
  only stamps the release)."""
  out = []
  for commit in log_json.get("log") or []:
    message = commit.get("message") or ""
    subject = message.split("\n", 1)[0].strip()
    if re.match(r"^Version \d+\.\d+\.\d+\.\d+$", subject):
      continue
    bugs = []
    for line in message.splitlines():
      m = re.match(r"^(?:Bug|Fixed):\s*(.+)$", line.strip())
      if m:
        bugs += [
          b.strip().split(":")[-1] for b in m.group(1).split(",") if b.strip()
        ]
    out.append((commit["commit"][:10], subject, bugs,
                gitiles_time(commit["committer"]["time"])))
  out.reverse()
  return out


def lag_days(oldest_missing, now):
  """How many whole days the oldest merge the pin lacks has been out."""
  return int((now - oldest_missing).total_seconds() // 86400)


def merge_lines(merge_list):
  lines = []
  for sha, subject, bugs, _ in merge_list:
    bug_text = f" (crbug {', '.join(bugs)})" if bugs else ""
    lines.append(f"- `{sha}` {subject}{bug_text}.")
  return lines


def release_notes(old_version, new_version, new_crate, branch, head_date,
                  merge_list, header_changes):
  api = ("changes no public API (V8's `DEPS` and headers are unchanged "
         "apart from `v8-version.h`)" if not header_changes else
         "changes V8's public headers: " +
         ", ".join(f"`{h}`" for h in header_changes) +
         " (check the bindings before releasing)")
  return "\n".join([
    f"V8 moves from {old_version} to **{new_version}**, the head of "
    f"`branch-heads/{branch}` on {head_date}; the crate is `{new_crate}`. "
    f"It adds {len(merge_list)} merge{'s' if len(merge_list) != 1 else ''} "
    f"and {api}:",
    "",
    *merge_lines(merge_list),
    "",
  ])


def denext_md_section(tag, old_version, new_version, old_sha, new_sha, branch,
                      head_date, new_crate, merge_list):
  return "\n".join([
    f"## `{tag}`: V8 {new_version}",
    "",
    f"A patch-level move on `branch-heads/{branch}`, at its head on "
    f"{head_date}: the `v8` submodule moves from {old_version} "
    f"(`{old_sha[:10]}`) to {new_version} (`{new_sha[:10]}`), by "
    f"tools/denext_patch_bump.py. The merges between them:",
    "",
    *merge_lines(merge_list),
    "",
    f"V8's `DEPS` did not change, and the `denext/patches/v8` series applies "
    f"unchanged. The crate is `{new_crate}`.",
    "",
    "",
  ])


def insert_denext_section(text, section):
  """`section` before DENEXT.md's newest release section."""
  i = text.find("\n## `v")
  if i < 0:
    raise ValueError("no release section in DENEXT.md")
  return text[:i + 1] + section + text[i + 1:]


def pr_body(tag, old_version, new_version, lag, merge_list, header_changes,
            notes_path):
  lines = [
    f"Moves `v8` from {old_version} to {new_version} (the head of its "
    f"branch) and the crate to `{tag}`.",
    "",
  ]
  if lag is not None:
    state = ("past" if lag > PATCH_LAG_DAYS else "within")
    lines += [
      f"The pin has lacked the oldest of these merges for {lag} days, "
      f"{state} the deno fork's {PATCH_LAG_DAYS}-day patch-lag gate.",
      "",
    ]
  if header_changes:
    lines += [
      "**Public headers changed:** " +
      ", ".join(f"`{h}`" for h in header_changes) +
      ". Check the bindings (`src/binding.cc`) before merging.",
      "",
    ]
  lines += [
    "Merges:",
    "",
    *merge_lines(merge_list),
    "",
    f"Release notes: `{notes_path}`. After the merge, dispatch "
    f"release-denext.yml with tag `{tag}` (a pull request opened with the "
    "workflow token starts no workflow run, so dispatch ci.yml on this "
    "branch by hand first).",
  ]
  return "\n".join(lines)


def run(argv, cwd=ROOT, check=True):
  print("$", " ".join(argv), flush=True)
  res = subprocess.run(argv, cwd=cwd, stdout=subprocess.PIPE,
                       stderr=subprocess.PIPE, text=True)
  if check and res.returncode != 0:
    sys.stderr.write(res.stderr)
    raise SystemExit(f"failed: {' '.join(argv)}")
  return res


def fetch_text(url):
  req = urllib.request.Request(url, headers={"User-Agent": "denext-ci"})
  with urllib.request.urlopen(req, timeout=TIMEOUT_SECONDS) as res:
    return res.read().decode("utf-8")


def read(path):
  with open(os.path.join(ROOT, path), encoding="utf-8") as f:
    return f.read()


def write(path, text):
  full = os.path.join(ROOT, path)
  os.makedirs(os.path.dirname(full), exist_ok=True)
  with open(full, "w", encoding="utf-8") as f:
    f.write(text)


def main(argv):
  p = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
  p.add_argument("--base", default="v8-15.6-denext")
  p.add_argument("--dry-run", action="store_true")
  p.add_argument("--now")
  args = p.parse_args(argv)
  now = (datetime.datetime.fromisoformat(args.now.replace("Z", "+00:00"))
         if args.now else datetime.datetime.now(datetime.timezone.utc))

  v8 = os.path.join(ROOT, "v8")
  old_sha = run(["git", "ls-tree", "HEAD", "v8"]).stdout.split()[2]
  # Read through gitiles: Chromium's git serves branch tips only, so the pin
  # (no longer a tip once its branch moved) cannot be fetched by itself.
  old_version = v8_version(
    base64.b64decode(fetch_text(V8_HEADER_URL.format(rev=old_sha))).decode())
  if old_version is None:
    raise SystemExit("the pinned v8-version.h has no version")
  branch = branch_of(old_version)
  ref = f"refs/branch-heads/{branch}"
  heads = run(["git", "ls-remote", V8_REMOTE, ref]).stdout.split()
  if not heads:
    raise SystemExit(f"{V8_REMOTE} has no {ref}")
  new_sha = heads[0]
  if new_sha == old_sha:
    print(f"V8 {old_version} is the head of branch-heads/{branch}; "
          f"nothing to do")
    return 0

  # The branch's recent history, which holds the pin (a submodule checkout,
  # or a repository of its own when the submodule is not initialized).
  if run(["git", "rev-parse", "--show-toplevel"], cwd=v8,
         check=False).stdout.strip() != os.path.realpath(v8):
    run(["git", "init", "--quiet", v8])
    run(["git", "remote", "add", "origin", V8_REMOTE], cwd=v8)
  run(["git", "fetch", f"--depth={BRANCH_DEPTH}", "origin",
       f"+{ref}:refs/remotes/origin/{branch}"], cwd=v8)
  if run(["git", "cat-file", "-e", f"{old_sha}^{{commit}}"], cwd=v8,
         check=False).returncode != 0:
    raise SystemExit(
      f"the pin {old_sha[:10]} is not in the last {BRANCH_DEPTH} commits of "
      f"{ref}")
  new_version = v8_version(
    run(["git", "show", f"{new_sha}:include/v8-version.h"], cwd=v8).stdout)
  if new_version is None:
    raise SystemExit("the head's v8-version.h has no version")
  print(f"branch-heads/{branch}: {old_version} -> {new_version}")
  if run(["git", "diff", "--quiet", old_sha, new_sha, "--", "DEPS"], cwd=v8,
         check=False).returncode != 0:
    raise SystemExit(
      f"V8's DEPS changed between {old_version} and {new_version}: not a "
      "patch bump. Run tools/update_deps.py and move the other submodules by "
      "hand.")
  header_changes = [
    h for h in run(["git", "diff", "--name-only", old_sha, new_sha, "--",
                    "include/"], cwd=v8).stdout.split()
    if h != "include/v8-version.h"
  ]

  # The denext patch series must still apply to the head.
  run(["git", "checkout", "--quiet", new_sha], cwd=v8)
  patches = sorted(glob.glob(os.path.join(ROOT, "denext/patches/v8/*.patch")))
  if patches:
    res = run(["git", "apply", "--check", *patches], cwd=v8, check=False)
    if res.returncode != 0:
      sys.stderr.write(res.stderr)
      raise SystemExit(f"denext/patches/v8 does not apply to {new_version}")

  log = gitiles_json(fetch_text(V8_LOG_URL.format(old=old_sha, new=new_sha)))
  merge_list = merges(log)
  head_time = max(
    (gitiles_time(c["committer"]["time"]) for c in log.get("log") or []),
    default=now)
  head_date = head_time.date().isoformat()
  lag = lag_days(merge_list[0][3], now) if merge_list else None
  if lag is not None:
    print(f"the pin has lacked {merge_list[0][0]} for {lag} days "
          f"(the deno fork's gate allows {PATCH_LAG_DAYS})")

  old_crate = crate_version(read("Cargo.toml"))
  new_crate = bump_crate_version(old_crate)
  old_tag, tag = release_tag(old_crate), release_tag(new_crate)
  notes_path = f"denext/release-notes/{tag}.md"
  write("Cargo.toml", bump_cargo_toml(read("Cargo.toml"), old_crate,
                                      new_crate))
  write("Cargo.lock", bump_cargo_lock(read("Cargo.lock"), old_crate,
                                      new_crate))
  write("README.md", bump_readme(read("README.md"), new_version))
  write(RELEASE_WORKFLOW,
        bump_release_workflow(read(RELEASE_WORKFLOW), old_tag, tag))
  write(notes_path, release_notes(old_version, new_version, new_crate, branch,
                                  head_date, merge_list, header_changes))
  write("DENEXT.md", insert_denext_section(
    read("DENEXT.md"),
    denext_md_section(tag, old_version, new_version, old_sha, new_sha, branch,
                      head_date, new_crate, merge_list)))
  body = pr_body(tag, old_version, new_version, lag, merge_list,
                 header_changes, notes_path)
  title = f"denext: V8 {new_version} (crate {new_crate})"
  if args.dry_run:
    print(f"--dry-run: files written, nothing committed\n\n{title}\n\n{body}")
    return 0

  head_branch = f"denext/v8-{new_version}"
  run(["git", "checkout", "-b", head_branch])
  run(["git", "add", "v8", "Cargo.toml", "Cargo.lock", "README.md",
       RELEASE_WORKFLOW, notes_path, "DENEXT.md"])
  run(["git", "commit", "-m", f"{title}\n\n{body}"])
  run(["git", "push", "origin", f"HEAD:refs/heads/{head_branch}"])
  open_prs = json.loads(
    run(["gh", "pr", "list", "--state", "open", "--head", head_branch,
         "--json", "number"]).stdout or "[]")
  if open_prs:
    run(["gh", "pr", "edit", str(open_prs[0]["number"]), "--title", title,
         "--body", body])
  else:
    run(["gh", "pr", "create", "--base", args.base, "--head", head_branch,
         "--title", title, "--body", body])
  return 0


if __name__ == "__main__":
  sys.exit(main(sys.argv[1:]))
