A maintenance release with a focus on display and interaction correctness: queue rows render cleanly, diagnostics tell the truth, and several silent failure paths were removed. No new features, no settings or workflow changes.

## Fixed

- **Queue rows no longer overlap each other.** Rows were laid out into fixed-height slots while their real height varies (titles, error lines, action rows), so every row painted partly over the next one and long lists clipped their tail. The list now sizes each row to its content.
- **"Check File Server" no longer reports a healthy server as not running.** The self-check used to probe loopback, but the server deliberately binds the network interface that can reach the console — so the probe was refused while a download was streaming. The check now targets the address the server actually listens on.
- **The transfer speed readout no longer freezes at 0 on fast polling settings.** The speed sample baseline was refreshed on every poll, so at intervals under 0.4 s the samples were never far enough apart to be used. The baseline now only advances when a sample is actually consumed.
- **Installing while switching consoles stays honest.** Task and job ids are console-specific: an item now remembers which console its task belongs to, so reconnecting, resuming or cancelling never aims an id at a different console's unrelated download. A late install answer after a disconnect keeps its id for re-attaching instead of stopping the task.
- **Opening a second file dialog no longer silently discards the first pick.** The "Add…" buttons stayed clickable while a dialog was already open; picking files in the first dialog and opening a second threw the confirmed selection away without any feedback. The buttons now wait for the open dialog.
- **A search that filters everything out no longer claims the queue is empty.** It showed the "drop files here" empty state as if the items had vanished; it now says nothing matches and offers to clear the search.
- **"Scan again" and Connect no longer stack duplicate network sweeps** while one is already running (including the automatic startup scan).
- **Consoles running on a custom port are re-validated correctly.** Remembered consoles were only probed on the two standard ports during a scan, so a console you had connected to on its own port silently failed re-validation. The scan now probes that port too.
- **The queue state is flushed to disk before the remote cleanup on exit** — previously a hung console could delay the final flush past the app's shutdown window.

## Internal

- Test suite: 100 automated tests (was 94) — all self-contained against a mock console, no console needed.
- `cargo fmt --check` and `cargo clippy -D warnings` clean; `unsafe` remains denied workspace-wide.

**Install:** download the archive for your platform, extract it next to your packages, and run the executable — the app is portable and stores its settings in the same folder.
