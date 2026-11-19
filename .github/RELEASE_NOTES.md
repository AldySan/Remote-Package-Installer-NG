A correctness release for package validation. The desktop client now recognizes real PS4 packages: the PKG magic check accepts the canonical console signature, and PARAM.SFO fields parse with the endianness the format actually uses, so files that install cleanly on the console no longer fail validation in the app. No settings or workflow changes.

## Fixed

- **Real PS4 packages pass validation.** The client checked PKG files for the wrong magic (`7F PKG`). Actual PS4 packages, including the console app's own shipped `.pkg`, start with `7F CNT`, the same signature the console app's parser accepts. Validation now matches what the console installs (PR #32).
- **PARAM.SFO parses with the correct endianness.** The SFO header and entry fields were read as big-endian; the format is little-endian, as the console app's parser implements. Titles and metadata from real packages now read correctly instead of failing or decoding to garbage.
- **Tests pin the real format.** The valid-PKG fixture is built from the canonical magic bytes instead of the implementation constant, and new tests accept the canonical signature while rejecting the old incorrect one, so a regression cannot hide behind a fixture that mirrors the bug.

## Internal

- Workspace version 0.0.10.

**Install:** download the archive for your platform, extract it next to your packages, and run the executable. The app is portable and stores its settings in the same folder.
