## Summary

<!-- What changed, and why. One or two short paragraphs; the details belong in the diff. -->

## Related issues

<!-- Issue numbers this closes, if any, for example: Closes #12 -->
<!-- A PR without an issue is fine too: just describe the motivation above. -->

## Type of change

- [ ] Bug fix
- [ ] New feature
- [ ] Documentation
- [ ] Refactoring (no behavior change)
- [ ] Other:

## How was this tested?

<!-- What you ran and what you saw, enough for a reviewer to repeat it.
     For code changes that is usually: -->

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

<!-- If your change is not covered by an automated test, say so and explain
     how you verified it by hand. -->

## Checklist

- [ ] The change is focused on one topic
- [ ] `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace --locked` all pass locally
- [ ] Behavior changes carry a test that would have failed before
- [ ] I did not modify unrelated code
- [ ] Documentation was updated where the change is user-visible
