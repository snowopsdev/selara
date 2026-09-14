#!/usr/bin/env python3
"""Copy local dotenv files from the main checkout into a new worktree."""

import os
from pathlib import Path
import shutil
import subprocess


def git_path(*args):
    return Path(subprocess.check_output(["git", *args], text=True).strip()).resolve()


def copy_env(source, destination):
    if source == destination:
        return 0

    copied = 0
    for directory, directories, files in os.walk(source):
        directories[:] = [
            name for name in directories
            if name not in {".git", "node_modules", "target", "dist", ".venv", "venv"}
            and not (Path(directory) / name).is_symlink()
            and (Path(directory) / name).resolve() != destination
        ]
        for name in files:
            if name != ".env" and not name.startswith(".env."):
                continue
            original = Path(directory) / name
            relative = original.relative_to(source)
            if original.is_symlink() or not original.is_file():
                continue
            # Tracked examples already arrive through Git.
            if subprocess.run(
                ["git", "-C", str(source), "check-ignore", "--quiet", "--", str(relative)],
                check=False,
            ).returncode != 0:
                continue
            target = destination / relative
            # Never follow a destination symlink outside this worktree.
            if any(parent.is_symlink() for parent in target.parents if parent != destination):
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            try:
                descriptor = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            except FileExistsError:
                continue
            with os.fdopen(descriptor, "wb") as output, original.open("rb") as incoming:
                shutil.copyfileobj(incoming, output)
            copied += 1
    return copied


if __name__ == "__main__":
    destination = (
        Path(os.environ["ORCA_WORKTREE_PATH"]).resolve()
        if os.environ.get("ORCA_WORKTREE_PATH") else git_path("rev-parse", "--show-toplevel")
    )
    source = (
        Path(os.environ["ORCA_ROOT_PATH"]).resolve()
        if os.environ.get("ORCA_ROOT_PATH")
        else git_path("rev-parse", "--path-format=absolute", "--git-common-dir").parent
    )
    if not source.is_dir() or not destination.is_dir():
        raise SystemExit("Both the main checkout and destination worktree must exist")
    count = copy_env(source, destination)
    print(f"Copied {count} environment file(s) from the main checkout; existing files preserved.")
