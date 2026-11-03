A maintenance release with two queue-reliability fixes — no new features, no settings or workflow changes.

## Fixed

- **Switching to a different console while a package was installing no longer leaves the app in a mixed state.** The progress display used to keep following the old console while pause/stop commands went to the new one — and the package's download was cut by the connection change without any indication. The in-flight package is now clearly marked "Unknown" with a hint to reconnect to the original console to re-attach and verify, the same honest handling an explicit disconnect gets. Switching consoles never cancels the install that is still running on the original console.
- **Pressing Retry gives a package its full set of automatic retries again.** If a package had already used up its retry budget (for example, the installer app on the console was still starting up), every later Retry would fail permanently on the first hiccup instead of retrying. Each fresh attempt now starts with a clean budget.

## Internal

- Test suite: 94 automated tests (was 92) — all self-contained against a mock console, no console needed. The two new tests lock in console switching and retry budgets.

**Install:** download the archive for your platform, extract it next to your packages, and run the executable — the app is portable and stores its settings in the same folder.
