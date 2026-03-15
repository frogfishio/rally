# SPDX-FileCopyrightText: 2026 Alexander R. Croft
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import argparse
import pathlib
import shutil
import sys


def main() -> int:
    parser = argparse.ArgumentParser(description="Package a Rally release bundle.")
    parser.add_argument("--target-dir", required=True, help="Output directory for the packaged release bundle.")
    parser.add_argument("--label", required=True, help="Platform label for informational output.")
    args = parser.parse_args()

    repo_root = pathlib.Path(__file__).resolve().parent.parent
    target_dir = repo_root / args.target_dir
    bin_dir = target_dir / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)

    binary_name = "rally.exe" if sys.platform.startswith("win") else "rally"
    source_binary = repo_root / "target" / "release" / binary_name
    if not source_binary.exists():
        raise FileNotFoundError(f"release binary not found: {source_binary}")

    shutil.copy2(source_binary, bin_dir / binary_name)

    for source_name in ("USER_GUIDE.md", "README.md", "LICENSE", "rally.toml.example"):
        shutil.copy2(repo_root / source_name, target_dir / source_name)

    print(f"Packaged Rally release bundle for {args.label} at {target_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
