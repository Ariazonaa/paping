#!/usr/bin/env python3
"""Prints the license texts of every crate that is linked into the paping binary.

    python3 scripts/third-party-licenses.py [--target TRIPLE] > THIRD-PARTY-LICENSES.txt

Only normal dependencies count: test and build dependencies are not shipped. With
--target, only the crates used on that platform are listed. Identical license texts are
printed once, followed by the crates that use them.
"""

import hashlib
import json
import pathlib
import subprocess
import sys

LICENSE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT")


def shipped_packages(target):
    # `cargo tree` resolves features the way the build does; `cargo metadata` would also
    # list optional crates that are never compiled. Metadata only supplies the paths.
    tree = subprocess.check_output(
        [
            "cargo", "tree", "--locked", "--edges", "normal", "--prefix", "none",
            "--format", "{p}", "--target", target or "all",
        ],
        text=True,
    )
    wanted = set()
    for line in tree.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[1].startswith("v"):
            wanted.add((parts[0], parts[1][1:]))
    meta = json.loads(
        subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--locked"])
    )
    root = meta["resolve"]["root"]
    packages = [
        p
        for p in meta["packages"]
        if (p["name"], p["version"]) in wanted and p["id"] != root
    ]
    return sorted(packages, key=lambda p: (p["name"], p["version"]))


def license_files(package):
    directory = pathlib.Path(package["manifest_path"]).parent
    files = [
        path
        for path in sorted(directory.iterdir())
        if path.is_file() and path.name.upper().startswith(LICENSE_PREFIXES)
    ]
    if package.get("license_file"):
        extra = directory / package["license_file"]
        if extra.is_file() and extra not in files:
            files.append(extra)
    return files


def main():
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    target = None
    if len(sys.argv) == 3 and sys.argv[1] == "--target":
        target = sys.argv[2]
    elif len(sys.argv) != 1:
        sys.exit(__doc__)

    texts = {}  # hash -> (text, [crate labels])
    missing = []
    packages = shipped_packages(target)
    for package in packages:
        label = f"{package['name']} {package['version']} ({package.get('license') or 'see text'})"
        files = license_files(package)
        if not files:
            missing.append((package, label))
        for path in files:
            text = path.read_text(encoding="utf-8", errors="replace").strip()
            key = hashlib.sha256(text.encode()).hexdigest()
            texts.setdefault(key, (text, []))[1].append(f"{label}, file {path.name}")

    out = sys.stdout
    out.write("THIRD-PARTY LICENSES\n\n")
    out.write(
        "paping includes the following Rust crates"
        + (f" when built for {target}" if target else "")
        + ". Their license texts follow.\n"
        + "paping itself is MIT licensed, see LICENSE; see NOTICE for paping 1.5.5.\n\n"
    )
    for package in packages:
        out.write(f"  {package['name']} {package['version']}  {package.get('license') or ''}\n")
    for text, users in texts.values():
        out.write("\n" + "=" * 78 + "\n")
        out.write("Used by:\n")
        for user in users:
            out.write(f"  {user}\n")
        out.write("-" * 78 + "\n\n")
        out.write(text + "\n")
    for package, label in missing:
        out.write("\n" + "=" * 78 + "\n")
        out.write(f"Used by:\n  {label} (the crate ships no license file)\n")
        out.write("-" * 78 + "\n\n")
        if "MIT" in (package.get("license") or ""):
            authors = ", ".join(package.get("authors") or []) or f"the {package['name']} authors"
            out.write(MIT.format(authors=authors) + "\n")
        else:
            out.write(f"Licensed under {package.get('license')}.\n")
            print(f"warning: no license text for {label}", file=sys.stderr)


MIT = """MIT License

Copyright (c) {authors}

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE."""


if __name__ == "__main__":
    main()
