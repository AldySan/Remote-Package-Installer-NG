A maintenance release: a second full re-audit of the v0.0.4 code base found six real issues (and nothing else worth changing) — all fixed here, with regression tests. No new features, no settings or workflow changes.

## Fixed

- **A second "Start queue" press while the console was still accepting a package could start two installs at once.** Between sending the request and the console's answer there was no guard, so a second Start (or a finished background scan, or a Retry) submitted the next package too. The second install silently took over monitoring and the first package could sit at "Transferring" forever. Installs are now strictly serialized: the queue waits for the console to answer before submitting anything else.
- **Cancelling a package while its request was still in flight did nothing.** The button looked pressed, but the install went ahead anyway once the console answered. Cancelling now sticks, and the task the console created for that late answer is stopped automatically.
- **A corrupted or truncated package could silently lose its metadata** (title, size, content id). A malformed PARAM.SFO value entry could crash the background parser, which swallowed the error and left the item without details. Malformed entries are now skipped safely.
- **The file server slowly accumulated duplicate registrations** on every submit/retry of the same package — a small, steady memory creep over a long session, plus stale download URLs staying alive. Re-registering the same file now reuses its existing entry.
- **The transfer speed readout jittered on classic (legacy) consoles.** The smoothing was wired to a field the console never fills, so every poll showed a raw spike. Speeds now blend with the previous sample, like the network rates do.
- **Activity log timestamps are now marked "UTC"** — they were always UTC, but unlabeled, so anyone outside UTC+0 read times hours off their wall clock with no hint.

## Internal

- Test suite: 92 automated tests (was 85) — all self-contained against a mock console, no console needed. Two of the new tests reproduce the submit-window races deterministically via a delayed mock answer.
- `cargo fmt --check` and `cargo clippy -D warnings` clean; `unsafe` remains denied workspace-wide.

**Install:** download the archive for your platform, extract it next to your packages, and run the executable — the app is portable and stores its settings in the same folder.
