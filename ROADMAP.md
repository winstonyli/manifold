# Roadmap

Ordered milestones, each a question the release answers. Tagging moves the line to `CHANGELOG.md`. Deferred work is in `PARKED.md`.

- **v0.10 - Can a user edit a document that lives in the synced document?** Properties plan 4b: the write path for inline Json cards (edit a scalar, add and delete keys, insert and reorder array elements, change a type; one undo step each), the Export action, `Ref` chips, and the two parked sync hazards that need order keys (`between`). 4a (read side, New JSON, Detach to inline) is built and untagged. Spec: `docs/superpowers/specs/2026-10-24-properties-design.md` section 4.
