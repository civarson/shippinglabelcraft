#!/usr/bin/env bash
# Release notes for tag $1 (e.g. v0.5.1): that version's CHANGELOG.md section, then downloads.
set -euo pipefail
ver="${1#v}"
section=$(awk -v v="$ver" '
  $0 ~ "^## \\[" v "\\]" { on=1; next }
  on && /^## \[/ { exit }
  on && /^\[[0-9]/ { exit }
  on { print }
' CHANGELOG.md)
if [ -z "$(echo "$section" | tr -d '[:space:]')" ]; then
  echo "CHANGELOG.md has no section for $ver; add '## [$ver] - date' before releasing." >&2
  exit 1
fi
echo "## What's new"
echo "$section"
cat <<'MD'

## Download
- **Windows (x64):** `labelcraft-windows-x64.zip` → `LabelCraft.exe`
  SmartScreen will warn because the app isn't code-signed: click **More info → Run anyway**.
  If it says `VCRUNTIME140.dll` is missing, install the Microsoft Visual C++ Redistributable (x64).
- **macOS (Apple Silicon and Intel):** `labelcraft-macos-universal.zip` → `LabelCraft.app`
  Right-click → **Open** the first time (the app is ad-hoc signed).
- **Linux (x86_64):** `labelcraft-linux-x86_64.AppImage` (`chmod +x`, then run it), or
  `labelcraft-linux-x86_64.tar.gz` with the plain binary. Printing uses CUPS.

## Using it
Drop a label PDF or image on the window, check that the barcodes list is green, pick your printer, and click **Print label**.
If the size or position is off, set the printer's default paper to the label size (4 × 6 in) in its driver settings.

Full history: [CHANGELOG.md](https://github.com/civarson/labelcraft/blob/main/CHANGELOG.md)
MD
