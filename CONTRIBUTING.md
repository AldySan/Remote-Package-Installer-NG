# Contributing to Remote Package Installer NG

Thank you for wanting to help. Whether you found a typo or want to build a
whole feature, contributions of every size are welcome, and this guide is
written so that a first-time contributor can follow it end to end.

If anything here is unclear or does not work for you, that is a bug in this
document. Please open an issue about it.

## Ways to help

You do not have to write code to contribute:

- **Report bugs.** A clear report with steps to reproduce is worth a lot.
- **Improve the docs.** If you had to figure something out the hard way,
  others will too.
- **Reproduce open issues.** Confirming a bug on your setup (and saying
  what that setup is) narrows it down.
- **Answer questions** in issue threads when you know the answer.
- **Write code.** The [issues page](https://github.com/AldySan/Remote-Package-Installer-NG/issues)
  always has something pickable; look for the labels.

Please also read the [Code of Conduct](CODE_OF_CONDUCT.md) so we all keep
the project a pleasant place.

## Getting set up

You need [Rust](https://rustup.rs) 1.95 or newer. Nothing else: the GUI
stack and all other dependencies are plain Cargo dependencies.

```sh
git clone https://github.com/AldySan/Remote-Package-Installer-NG.git
cd Remote-Package-Installer-NG

cargo build --workspace          # first build takes a few minutes
cargo test --workspace --locked  # the whole suite, no PS4 needed
```

The test suite is self-contained: a mock console implements the wire
protocol, and PKG fixtures are generated on the fly. See
[docs/TESTING.md](docs/TESTING.md) for what it covers.

## Before you start coding

1. Check the [issues](https://github.com/AldySan/Remote-Package-Installer-NG/issues)
   for an existing report or plan. If your idea is new or large, open an
   issue first so we can agree on the direction before you spend time on it.
2. Keep the change focused. One pull request per topic is much easier to
   review than several topics mixed together.

## The development workflow

The `dev` branch is where day-to-day work lands, and `main` is reserved for
releases. The golden rule: **never commit directly to `main` or `dev`**.

1. Make sure you are on the latest `dev`:

   ```sh
   git switch dev
   git pull
   ```

2. Create a branch for your change. Short, lowercase, hyphenated names that
   say what it is:

   ```sh
   git switch -c fix/queue-pause-button
   git switch -c feat/ps5-support        # (someday)
   git switch -c docs/build-instructions
   ```

3. Make your change, commit as you go (see the commit style below).

4. Before opening the pull request, run the three checks CI will run:

   ```sh
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace --locked
   ```

5. Push your branch and open a pull request **into `dev`** (not `main`).
   Fill in the template; it is short.

6. After review, the pull request is merged into `dev`. When a release is
   cut, `dev` is squash-merged into `main` as a single release commit, so
   `main` stays a clean changelog.

If your branch is no longer needed after merging, delete it. If you worked
on something and the plan changed, closing a pull request is fine: just
leave a sentence about why.

## Commit style

Write commit messages in English, in the conventional format already used
by the project history: a short, specific first line, optionally a body
explaining the why.

```
fix(ui): keep the progress readout inside its column

The progress bar claims all remaining row width, so the size/speed
label placed after it painted past the fixed column edge.
```

Common prefixes you will see in the history:

| Prefix | Use it for |
| --- | --- |
| `feat:` | New user-facing behavior |
| `fix:` | Bug fixes |
| `docs:` | Documentation only |
| `refactor:` | Restructuring without behavior change |
| `test:` | Test-only changes |
| `chore:` | Tooling, release preparation, housekeeping |

A scope in parentheses (`fix(ui):`, `docs(readme):`) is welcome when it
helps, but not required.

## Code style

The boring parts are enforced by tools, so you can focus on the code:

- **Formatting** is `rustfmt`, no discussion. Run `cargo fmt --all` before
  committing.
- **Warnings are errors** in CI. `cargo clippy --workspace --all-targets
  -- -D warnings` must pass.
- **Comments** are for the *why* of non-obvious code, in the same sparse
  style the codebase already uses. If a comment would restate the code,
  leave it out.
- **Tests** come with behavior changes. The project convention is that a
  bug fix carries the test that would have caught it. Pure logic is
  extracted into testable pieces; see `RowActions` in `queue_view.rs` for
  an example of the pattern.
- **Keep diffs focused.** Do not delete unrelated code or "tidy up" files
  you are not otherwise touching. A reviewer should see exactly your topic.

## Reporting bugs

Use the bug report template when you open an issue. The more of it you fill
in, the faster things get fixed. Two things help most of all:

- **The activity log.** Copy the lines from the bottom panel that cover the
  moment things went wrong. It is small, readable and timestamped.
- **What actually happened vs. what you expected.** "It didn't work" is
  hard to act on; "the item stayed at 40% and the log said X" is gold.

Thank you for reading this far, and for helping the project.
