A look-and-feel release. The app now ships its own icon on every platform, every status color stays readable in both light and dark themes, and the whole interface keeps working at its smallest documented window size. No settings or workflow changes.

## Added

- **The app has its own icon everywhere.** After v0.0.8 the Windows `.exe` still showed the generic executable icon in Explorer, and macOS showed none at all. The app now embeds a brand icon in the binary, so it appears on the `.exe` file, taskbar and title bar on Windows; on Linux the tarball ships a `.desktop` entry plus launcher icons; and on macOS the release archive is now a proper `RemotePackageInstallerNG.app` bundle (universal binary, icon in Finder, Dock and cmd-tab). The first-run note about unsigned downloads still applies on macOS.
- **A refreshed README.** Real screenshots of the running app, release and platform badges, and a support section.

## Fixed

- **Status colors pass WCAG AA in both themes.** All status text (ok, warning, error, info, installing, neutral) now comes from one semantic palette with a light and a dark value each, and unit tests compute the real contrast ratios against the theme backgrounds so nothing ships below 4.5:1. The dim helper text in the dark theme was raised from about 2.3:1 to 4.7:1.
- **Small windows no longer collide.** At the minimum window size (860x560) the queue toolbar, the top bar and the file-server strip now wrap onto a second row instead of painting over each other; an over-long console status chip truncates instead of running under the buttons; and the connect view scrolls when its content is taller than the panel, so the manual address field and the troubleshooting section are always reachable.
- **Transfer readouts stay inside their column.** The transferred/total, speed and ETA line under each progress bar used to paint past the edge of its column and get clipped while a package was transferring; it now always fits.
- **Escape closes the front-most window.** Settings, About, Software update and Add folder now close on Escape, and when several windows are stacked only the front one closes.
- **Up/Down buttons are honest at the queue edges.** "Up" on the first row and "Down" on the last row render disabled with a hover explanation ("Already at the top of the queue") instead of looking clickable and doing nothing.
- **Punctuation sweep.** The remaining em dashes in user-facing strings, activity-log lines and error messages are gone, and a source-hygiene test now fails the build if one reappears in a string literal.

## Internal

- Test suite: 148 automated tests (was 124). New tests pin the contrast ratios, measure the real widget widths at the minimum window, cover Escape handling with stacked windows, and scan every source file for em dashes in string literals.
- The repo gains a code of conduct, a contributing guide and issue/PR templates; DESIGN.md records the interface direction; MIGRATION.md now documents that data lives next to the executable.
- `cargo fmt --check` and `cargo clippy -D warnings` clean; `unsafe` remains denied workspace-wide.

**Install:** download the archive for your platform, extract it next to your packages, and run the executable. The app is portable and stores its settings in the same folder.
