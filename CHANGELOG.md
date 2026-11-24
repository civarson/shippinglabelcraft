# Changelog

All notable changes to LabelCraft. Each release on GitHub uses its section from this file
as its release notes.

## [0.5.2] - 2026-11-24

### Added
- **Driver capture** (*Settings → Printing → Capture what the driver sends*): runs one
  print through the printer's driver and saves the bytes it would send, without printing.
  This is the first step towards printing to PeriPage printers directly, without the
  driver. Command line: `--capture <printer> <file>`.
- A tip when the printer driver uses different horizontal and vertical resolutions, such as
  PeriPage's "HD" drivers. The "SD" driver gives sharper barcodes.

## [0.5.1] - 2026-11-24

### Added
- App icon for the Windows `.exe` (Explorer, the desktop and shortcuts), the macOS app
  bundle, and every open window. Before, Windows only showed it on the taskbar.
- This changelog. Release notes now come from it.

## [0.5.0] - 2026-11-24

### Added
- **Printer profiles** for PeriPage PB41, Zebra, Rollo, MUNBYN, iDPRT / Polono, Jadens,
  Nelko, Phomemo, DYMO LabelWriter 4XL / 5XL and Brother QL-1100 / 1110. LabelCraft
  recognises the printer from its name and sets the resolution (203 or 300 dpi) and the
  print width. You can also pick the model by hand in *Settings → Printing*. Only the PB41
  has been tested on hardware; the others come from manufacturer specs.
- **Direct printing (experimental, off by default):** send the label as a ZPL or TSPL job
  instead of going through the printer driver. *Save a test job* writes the job to a file.
- **Linux downloads:** an AppImage and a `.tar.gz`.
- **Universal macOS app** that runs on both Apple Silicon and Intel Macs.
- Command line: `--dpi`, `--zpl` and `--tspl`.

### Fixed
- eBay labels: the restriction notice and 2D code printed under the label's border are now
  kept, and the code is redrawn and verified instead of being cut off.
- At 300 dpi, barcodes from blurry low-resolution sources are still found and redrawn.

## [0.4.1] - 2026-11-24

### Added
- **Undo and redo** buttons in the title bar (Ctrl/⌘ Z, Ctrl Y / ⇧⌘Z) for rotation,
  manual crop, darkness, barcode redrawing, text edits and tracking checks.

### Fixed
- Postage and label IDs (long numbers that aren't in any barcode) no longer count as
  tracking numbers, and lines left as original print never ask for a double-check.
- PDFs no longer show half-cut content from outside the crop at the label's edges.

## [0.4.0] - 2026-11-24

### Added
- **Edit text in place** (with OCR on): click a line on the label to retype it. The new
  text keeps the line's size and weight, and is read back to check it.
- Numbers that are also in a barcode (like the ZIP) can't be changed.
- **Tracking-number double-check:** shows the original print, what will print, and whether
  it matches the barcode. Printing waits until you confirm.
- The *Text* card lists edits with undo for each line and *Revert all*.
- Command line: `--edit N=text`.

## [0.3.2] - 2026-11-24

### Fixed
- Hyphens and colons were mistaken for superscripts, so some lines kept their original
  print unnecessarily.

## [0.3.1] - 2026-11-24

### Changed
- Default edge margin is 0.5 mm (was 1.5 mm), so labels print about 2% larger.

### Fixed
- OCR redrew a superscript ™ as full-size "TM".

## [0.3.0] - 2026-11-24

### Added
- PDFs are drawn directly at the printer's resolution, so text is sharp.
- Opt-in OCR that redraws blurry text in a clean font, with strict safety checks and a
  review step before printing.
- Settings screen and 15 color themes.

## [0.2.0] - 2026-11-24

### Changed
- Redesigned interface.
- Every push to `main` builds Windows and macOS apps and publishes a release.

## [0.1.0] - 2026-11-23

### Added
- First release: auto-crop, straighten and rotate shipping labels; redraw and verify every
  barcode; print directly to the PeriPage PB41 on Windows and macOS.

[0.5.2]: https://github.com/civarson/labelcraft/releases/tag/v0.5.2
[0.5.1]: https://github.com/civarson/labelcraft/releases/tag/v0.5.1
[0.5.0]: https://github.com/civarson/labelcraft/releases/tag/v0.5.0
[0.4.1]: https://github.com/civarson/labelcraft/releases/tag/v0.4.1
[0.4.0]: https://github.com/civarson/labelcraft/releases/tag/v0.4.0
[0.3.2]: https://github.com/civarson/labelcraft/releases/tag/v0.3.2
[0.3.1]: https://github.com/civarson/labelcraft/releases/tag/v0.3.1
[0.3.0]: https://github.com/civarson/labelcraft/releases/tag/v0.3.0
[0.2.0]: https://github.com/civarson/labelcraft/releases/tag/v0.2.0
[0.1.0]: https://github.com/civarson/labelcraft/releases/tag/v0.1.0
