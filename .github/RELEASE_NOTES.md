A hardening release for the queue file. The queue lives next to the app as plain JSON, and a hand-edited or otherwise damaged copy could break the app in confusing ways — rows that answered to the wrong buttons, or entries that vanished. That file is now checked and repaired safely on startup, with every repair explained in the activity panel. No settings or workflow changes.

## Fixed

- **A hand-edited queue file no longer produces broken rows.** Editing `queue.json` and accidentally leaving two entries with the same id made the second row render fine while its buttons (Retry, Remove, reorder…) silently acted on the *first* item — its state would never change no matter what you clicked. On startup the app now keeps the first entry of each id, drops the duplicates, and says so in the activity log.
- **Renumbered entries can no longer corrupt the queue.** The counter at the top of the file is easy to leave behind when adding an entry by hand; the next package you added would then reuse an id already in use and take over that item's row. The counter is now verified against the entries and corrected on startup.
- **Entries without files are marked invalid instead of failing silently.** An entry with no files would previously be sent to the console as an install with nothing to download. It now stays visible in the queue as "Invalid package" with the reason, so you can remove it knowing what happened.
- **Repairs are visible, and untouched files stay untouched.** Every startup repair is logged to the activity panel — entries are never dropped silently. Files written by the app itself always load with zero changes.

## Internal

- Test suite: 124 automated tests (was 100). The interface state logic — which buttons appear in which queue state, which rows the search shows, when the toolbar buttons enable and why, and the speed/size/time readouts — is now covered by automated tests without needing a GUI harness. All tests are self-contained against a mock console.
- `cargo fmt --check` and `cargo clippy -D warnings` clean; `unsafe` remains denied workspace-wide.

**Install:** download the archive for your platform, extract it next to your packages, and run the executable — the app is portable and stores its settings in the same folder.
