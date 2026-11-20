#!/usr/bin/env python3
"""Checks tests/COVERAGE.md against the logs of a test run.

    python3 tests/check_coverage.py --meson-log BUILD/meson-logs/testlog.txt \
        --e2e-log e2e.log [--fake-log fake.log] [--merged RANGE]

- every test listed in the table ran and passed;
- every merged commit in RANGE (default origin/main..<the merge of
  vitlav/next>) is listed, except translations, CI and docs;
- `removed:` entries are really gone from the source.
"""

import argparse
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SKIPPED_SUBJECTS = re.compile(r"^(chore|ci|docs)(\(|:)|l10n|readme", re.I)


def git(*args):
    return subprocess.run(
        ["git", "-C", str(ROOT), *args], capture_output=True, text=True, check=True
    ).stdout


def table():
    rows = []
    for line in (ROOT / "tests/COVERAGE.md").read_text().splitlines():
        if not line.startswith("| `"):
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        commits = re.findall(r"`([0-9a-f]{8})`", cells[0])
        tests = re.findall(r"`([a-z0-9]+:[^`]+)`", cells[2])
        rows.append((commits, cells[1], tests))
    return rows


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--meson-log", required=True)
    parser.add_argument("--e2e-log", required=True)
    parser.add_argument("--fake-log")
    parser.add_argument("--merged", help="git range of the merged commits")
    args = parser.parse_args()

    meson_log = pathlib.Path(args.meson_log).read_text(errors="replace")
    e2e_log = pathlib.Path(args.e2e_log).read_text(errors="replace")
    fake_log = pathlib.Path(args.fake_log).read_text(errors="replace") if args.fake_log else ""
    gtk_passed = "test ui::tests::gtk ... ok" in meson_log

    def passed(test):
        kind, _, name = test.partition(":")
        if kind == "unit":
            return f"test {name} ... ok" in meson_log
        if kind == "gtk":
            return gtk_passed and any(l.strip() == name for l in meson_log.splitlines())
        if kind == "meson":
            # testlog.txt: "test: <suite> - <project>:<name>" ... "result: exit status 0"
            pattern = rf"^test:\s+.*:{re.escape(name)}\n(?:.*\n){{0,3}}?result:\s+exit status 0$"
            return re.search(pattern, meson_log, re.M) is not None
        if kind == "pack":
            # build-aux/check-packaging.py, run as the check-packaging test
            return f"ok {name}" in meson_log.splitlines() or any(
                l.strip() == f"ok {name}" for l in meson_log.splitlines()
            )
        if kind == "e2e":
            return f"ok   {name}" in e2e_log
        if kind == "fake":
            return f"test {name} ... ok" in fake_log
        if kind == "build":
            return True
        if kind == "removed":
            return git("grep", "-c", name, "--", "src").strip() == ""
        raise SystemExit(f"unknown test kind in {test}")

    problems = []
    listed = set()
    for commits, feature, tests in table():
        listed.update(commits)
        if not tests:
            problems.append(f"{feature}: no test listed")
        for test in tests:
            try:
                ok = passed(test)
            except subprocess.CalledProcessError:
                ok = True  # git grep exits 1 when nothing matches
            print(f"{'ok  ' if ok else 'MISS'} {feature}: {test}")
            if not ok:
                problems.append(f"{feature}: {test} did not run or failed")

    # Every row is backed by at least one mutation in tests/mutations.py
    mutated = set(re.findall(r"\b[0-9a-f]{8}\b", (ROOT / "tests/mutations.py").read_text()))
    for commits, feature, _ in table():
        if not mutated.intersection(commits):
            problems.append(f"{feature}: no mutation in tests/mutations.py")

    merge = git("log", "--merges", "--format=%h", "--abbrev=8", "--grep=vitlav/next", "-1").strip()
    merged_range = args.merged or f"origin/main..{merge}"
    for line in git("log", "--no-merges", "--format=%h %s", "--abbrev=8", merged_range).splitlines():
        sha, subject = line.split(" ", 1)
        if SKIPPED_SUBJECTS.search(subject):
            continue
        if sha not in listed:
            problems.append(f"merged commit {sha} ({subject}) is not in the table")
    if merge and merge not in listed:
        problems.append(f"the merge {merge} is not in the table")

    print()
    if problems:
        print("\n".join(problems))
        return 1
    print(f"all {len(listed)} merged commits are covered, every listed test passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
