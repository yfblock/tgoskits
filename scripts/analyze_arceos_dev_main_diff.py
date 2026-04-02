#!/usr/bin/env python3
"""Analyze dev-vs-main divergence for arceos-org repositories."""

from __future__ import annotations

import argparse
import csv
import json
import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
REPOS_CSV = REPO_ROOT / "scripts" / "repo" / "repos.csv"
USER_AGENT = "tgoskits-arceos-dev-main-diff/1.0"


@dataclass(frozen=True)
class RepoRecord:
    owner: str
    repo: str
    url: str
    category: str
    target_dir: str


@dataclass(frozen=True)
class CompareResult:
    owner: str
    repo: str
    ahead_by: int | None
    behind_by: int | None
    status: str
    detail: str


def normalize_github_repo(url: str) -> tuple[str, str] | None:
    prefix = "https://github.com/"
    if not url.startswith(prefix):
        return None
    path = url[len(prefix) :].strip().rstrip("/")
    if path.endswith(".git"):
        path = path[:-4]
    parts = path.split("/")
    if len(parts) < 2:
        return None
    owner, repo = parts[0], parts[1]
    return owner, repo


def run(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        cwd=str(cwd) if cwd else None,
        check=check,
        text=True,
        capture_output=True,
    )


def load_arceos_org_repos(csv_path: Path) -> list[RepoRecord]:
    seen: set[tuple[str, str]] = set()
    repos: list[RepoRecord] = []
    with csv_path.open(newline="", encoding="utf-8") as fh:
        reader = csv.DictReader(fh)
        for row in reader:
            url = (row.get("url") or "").strip()
            normalized = normalize_github_repo(url)
            if normalized is None:
                continue
            owner, repo = normalized
            if owner != "arceos-org":
                continue
            key = (owner, repo)
            if key in seen:
                continue
            seen.add(key)
            repos.append(
                RepoRecord(
                    owner=owner,
                    repo=repo,
                    url=url,
                    category=(row.get("category") or "").strip(),
                    target_dir=(row.get("target_dir") or "").strip(),
                )
            )
    return sorted(repos, key=lambda item: item.repo)


def compare_branches(owner: str, repo: str) -> CompareResult:
    repo_url = f"https://github.com/{owner}/{repo}.git"
    with tempfile.TemporaryDirectory(prefix=f"{repo}-") as tmpdir:
        tmp_path = Path(tmpdir)
        try:
            run(["git", "init"], cwd=tmp_path)
            run(["git", "remote", "add", "origin", repo_url], cwd=tmp_path)
            run(
                [
                    "git",
                    "fetch",
                    "--filter=blob:none",
                    "--no-tags",
                    "origin",
                    "refs/heads/main:refs/remotes/origin/main",
                    "refs/heads/dev:refs/remotes/origin/dev",
                ],
                cwd=tmp_path,
            )
            proc = run(
                [
                    "git",
                    "rev-list",
                    "--left-right",
                    "--count",
                    "refs/remotes/origin/main...refs/remotes/origin/dev",
                ],
                cwd=tmp_path,
            )
        except subprocess.CalledProcessError as exc:
            detail = (exc.stderr or exc.stdout or "").strip()
            if "couldn't find remote ref" in detail or "fatal: couldn't find remote ref" in detail:
                return CompareResult(owner, repo, None, None, "MISSING", detail)
            return CompareResult(owner, repo, None, None, "ERROR", detail or f"git exit {exc.returncode}")

    parts = proc.stdout.strip().split()
    if len(parts) != 2:
        return CompareResult(owner, repo, None, None, "ERROR", f"unexpected rev-list output: {proc.stdout.strip()}")
    try:
        behind_by = int(parts[0])
        ahead_by = int(parts[1])
    except ValueError:
        return CompareResult(owner, repo, None, None, "ERROR", f"unexpected rev-list output: {proc.stdout.strip()}")

    if ahead_by == 0 and behind_by == 0:
        status = "IDENTICAL"
    elif ahead_by > 0 and behind_by == 0:
        status = "AHEAD"
    elif ahead_by == 0 and behind_by > 0:
        status = "BEHIND"
    else:
        status = "DIVERGED"
    return CompareResult(owner, repo, ahead_by, behind_by, status, "")


def print_table(results: list[CompareResult]) -> None:
    print(f"{'repo':32} {'ahead(dev-main)':>15} {'behind(dev-main)':>16} status")
    print("-" * 80)
    for item in results:
        ahead = "-" if item.ahead_by is None else str(item.ahead_by)
        behind = "-" if item.behind_by is None else str(item.behind_by)
        status = item.status
        if item.detail:
            status = f"{status}: {item.detail}"
        print(f"{item.repo:32} {ahead:>15} {behind:>16} {status}")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Analyze dev-vs-main divergence for arceos-org repositories listed in scripts/repo/repos.csv."
    )
    parser.add_argument(
        "--csv",
        default=str(REPOS_CSV),
        help=f"Path to repos.csv. Default: {REPOS_CSV}",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Print JSON instead of a text table.",
    )
    args = parser.parse_args()

    csv_path = Path(args.csv).resolve()
    repos = load_arceos_org_repos(csv_path)
    if not repos:
        print(f"no arceos-org repositories found in {csv_path}", file=sys.stderr)
        return 1

    results: list[CompareResult] = []
    for repo in repos:
        results.append(compare_branches(repo.owner, repo.repo))

    if args.json:
        print(
            json.dumps(
                [
                    {
                        "owner": item.owner,
                        "repo": item.repo,
                        "ahead_by": item.ahead_by,
                        "behind_by": item.behind_by,
                        "status": item.status,
                        "detail": item.detail,
                    }
                    for item in results
                ],
                ensure_ascii=False,
                indent=2,
            )
        )
    else:
        print_table(results)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
