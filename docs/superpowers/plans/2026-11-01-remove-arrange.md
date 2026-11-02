---
status: done
---

# Remove Arrange by topic

Decided 2026-11-01 (user, as1 and au1; PARKED "No topic force in the settle"): topic structure comes only from Suggest
groups, so the explicit Arrange action goes. Pin/Unpin, Tidy and Pin All stay.

Done in one commit:
- Deleted `search/arrange.rs`, `graph_layout/prototype.rs` and the gate stage `gate/src/arrange.rs`; removed the toolbar
  chip, the omnibar row, the `ARRANGE` debug control and the gate's `arrange` step.
- `tidy_level` and `level_has_drops` moved next to `clear_anchors` in `session_sync/edits.rs`, with three tests
  (`session_sync/tests/edits.rs`).
- Docs: README, the 2026-11-01 spec's architecture check, PARKED (two entries closed, one path fixed), cleanup plan TEST8.

Checks: `cargo test -p manifold -p gate` 1876 passed, 0 failed, 17 ignored; `cargo fmt --check`; clippy `-D warnings`.
The full gate was not run (on request only).

The removed code: `git show cabfa377:crates/manifold/src/search/arrange.rs`.
