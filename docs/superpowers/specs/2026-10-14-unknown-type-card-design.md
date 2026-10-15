# The unknown-type card: design, 2026-10-14

Status: built 2026-10-14 (plan `../plans/2026-10-14-unknown-type-card.md`; deviations D1-D8 there). Design approved in
outline by the user on 2026-10-14 ("go" to: live card; Close and Reopen work; links allowed). Revised after an adversarial review (9 findings, all addressed below). Item 5 of
`2026-10-04-homogeneity-decisions.md`. Supersedes `2026-10-04-node-model-design.md` line 75 (the `*/*` row is
`portable` and `restorable` false), see decision 7.

## Goal

A node whose type this build does not know (a newer client's type, e.g. `application/x-from-the-future`) is kept in
the document and synced, but never shown: `Node::is_live()` is false for `Kind::Unknown`, `snapshot` returns `None` for
it and `insert_materialized` places nothing. It can't be seen, moved, grouped, linked, closed or deleted, and a session
that holds one looks emptier than it is. It becomes a plain **live card** with the generic behaviour every node has,
and nothing else.

## Decisions

1. **Live.** An unknown-type node is live when it has a type and is not deleted (`has_type() && !deleted`). A
   placeholder with no type yet stays hidden, as now. Every `is_live` consumer follows (`Doc::live`, `shown`,
   `restore_session`, `remote_changes`, `resolve_id`, `create_ops`' `in_doc`, `editable`, the journal reload diff,
   `store/json.rs` export and subtree export), so an unknown node is also exported and torn down with its session.
2. **Only a group is an effective group.** `effective_group` (`store/model.rs:515-530`) skips a parent only when it is
   not live. With unknown nodes live, a newer client's container type would become the group of its members. The
   store has no type-string accessor and must not depend on caps, so `Node::is_group()` (`kind() == Kind::Group`) is
   added and `effective_group` accepts a parent only when `is_live() && is_group()`; members of any other parent (an
   unknown type, and also a live Tab or Jar) are lifted to its nearest group, or the root. The document keeps their
   `GROUP` unchanged. `with_descendants` (`model.rs:534-548`) walks the raw `group` chain and needs no change. The
   comment above `effective_group` changes; the `model.rs:722-729` expectations hold.
3. **Generic fields only.** The app reads and writes five fields on it: `POS`, `COLOR`, `NAME`, `PARENT` ("opened
   from") and its group. Every other field stays raw in the document, untouched (2b-5 Ruling 2). A move, rename,
   recolor or regroup is an ordinary `Set` or `Move` op, so a newer client's fields survive them.
4. **A copy carries the generic fields and the type, not the raw extras.** *Superseded for Close and Reopen by
   `2026-10-14-closed-field-design.md`: a closed unknown node keeps its raw fields and reopens whole; only Duplicate
   and import make shells.* Duplicate, Reopen from Recently Closed and
   JSON import make a new node. A field number this build does not know has an unknowable type, so an `Id` inside it
   can't be remapped: copying it verbatim would alias the original node or a dead one. So a copy is a shell of the
   type. Close then Reopen likewise brings back the shell: Close tombstones the node in the document (its raw fields
   live on there) and Reopen always creates a new node with a fresh id (`reopen_closed`), as it does for every kind;
   nothing un-tombstones. Parked: a verbatim passthrough, until a real need appears.
5. **The type string is stored per node and travels with it.** `Kind` is `Copy` and has no payload, so it can't carry
   it. Every site that writes, snapshots or exports a type takes the node's actual string (`Mirror::ty`), never
   `Kind::type_str()` (`None` for Unknown). Concretely: `Snapshot`, `Imported`, `RemoteNode`/`RemoteChange` and
   `Carried` gain `ty: String`; `OpBody::create` takes the type as `&str`; `node_json` and `node_to_json` take it;
   `materialize` seeds the mirror's `TYPE` from it (`session_sync.rs:503` skips Unknown today); `place_node` takes it
   (`fetched.rs`). `fields_of`, `snapshot`, `imported_fields` and `create_ops` need no branch: `Kind::fields()` returns
   the generic list for Unknown (plan D1). An empty `type` string counts as no type (plan D8). A mirror's first-seen type string stands (`TYPE` is a last-writer register, but the store never
   changes a node's type, which `session_sync.rs:502` already relies on).
6. **A runtime of its own.** `Runtime::Unknown` (unit variant, like `Runtime::Jar`). `Runtime::None` already means
   "group" in several places, so it must not be reused. `named_by_name` (`session_sync.rs:955`) is left unchanged
   (plan D4): adding `Runtime::Unknown` would make the name the card's only label, refuse an empty rename and make
   `accessible_name` ignore the type. Like every non-group card, a stored `NAME` overrides the label and an empty one
   clears it; the title bar, filter text (`card.rs:1274`) and accessible name already read the stored name.
7. **One canonical type for rows and edges.** `card_kind` today maps a raw type to a row string, and `closed.rs:45/171`
   and `links.rs:116` pass the raw string instead, so the two disagree for an unknown type. One function,
   `card_kind::canonical_type(raw) -> &'static str`, maps a raw type to the row string a known kind uses, and anything
   else to a reserved sentinel `application/vnd.manifold.unknown`. Every call into `registry().caps` and
   `edges().*` goes through it.
   (`CARD_TYPES` is at `caps/edges.rs:39`.) The wildcard row `*/*` (`caps/rows.rs:98`, `restorable(false)`, layer 0) is replaced
   by `Row::new(SENTINEL).restorable(true).draw_layer(3)` (a `Row` has only those two capabilities; Rename, Pin and
   Duplicate come from the `_` arms of `node_menu.rs:651` and `card.rs:1307`, not from caps). A copy is a shell
   (decision 4), so the old reason for refusing it, unvalidated raw fields, is gone. Layer 3 sits with notes and jars
   (tabs 0 and 1, downloads 2, groups 7). `TYPE_DOWNLOAD` says `restorable(false)` itself instead of inheriting it. A
   raw type that matches no row falls to the `Caps` default; with the canonical function nothing reaches the registry
   unmapped. `closed.rs:171` already agrees with the sentinel (no `contains` target matches either string); `closed.rs:45`
   and `links.rs:116` are the callers that pass a raw string today.
8. **Edges.** The sentinel is added to the `contains` source list and to both `link` lists (`CARD_TYPES`,
   `caps/edges.rs:36`; `link` is symmetric). It is not a `contains` target, not an `opened-from` end and has no `jar`
   edge. `Pattern::Any` is not used for it: `*/*` also matches groups and jars, which `opened-from` and `jar`
   exclude on purpose.
9. **Look.** A card whose title bar shows the node's `NAME` if it has one, else the type string (the title bar, not
   the body, carries the label: `paint_card_body` paints nothing for it, like the other body-less cards); a size
   constant. Recently Closed rows (`popups.rs:728-732`, which would send an unknown kind to the Json arm) show the
   same text. Menu: the generic rows only (Rename, Duplicate, Pin, Color, Edges, Move to, Remove from Group, Delete).
10. **Retired types become visible.** `application/vnd.manifold.json-leaf` (retired; `store/journal/tests.rs:297`)
    and any other type an older build wrote are now unknown, so an existing session holding one shows a card for
    it. Accepted: no migration (`no-migration-until-users`), and Close removes it.

## Model

- `Node::is_live`: `!deleted && !ty().is_empty()` (an empty type string is no type, plan D8). `Kind::Unknown` keeps
  meaning "a type this build lacks".
- There is no `generic_fields()` (plan D1). `Kind::fields()` returns the generic list (`POS, COLOR, PARENT, NAME`, a
  `GENERIC` const) for Unknown, so everything that reads, mirrors, snapshots or copies "the kind's fields" handles an
  unknown node with no branch.
- `Node::get` and `Mirror::get` panic on a field number with no definition. The generic list holds only known
  numbers, so they are safe; a raw passthrough would not be, which is one more reason for decision 4.
- The mirror's `TYPE` for an unknown node is its real string. `card_kind` and `closed.rs`/`links.rs` agree through
  `canonical_type`. `EXPORTSTATE` keeps `type` as the canonical row string and adds `stored_type`, the raw string
  (plan D6). `CLOSENODE id` replaces `CLOSEGROUP id` so a gate can close an unknown card by id (plan D7).

## Behaviour

- **Sync.** `materialize` places the card; remote moves, renames and deletes take the generic path. A local move,
  rename, recolor or regroup writes only its own field. A remote delete removes the card (`drop_node` is generic). A
  node whose `TYPE` has not arrived stays hidden until it does; apply order is already safe (`apply_remote` sorts
  only groups first, and `kind()` is read at apply time).
- **Close and Reopen.** *Superseded for Close and Reopen by `2026-10-14-closed-field-design.md`: Close sets `closed`
  on the node and Reopen clears it; the node keeps its raw fields.* Close records the shell in Recently Closed (`LocalState.closed`, `JsonDoc` entries; its
  `"type"` key holds the real string) and `subtree_entry` stops skipping an unknown group member
  (`closed.rs:174`, via `snapshot`). Reopen creates a node of that type string with the generic fields.
- **Duplicate.** The same shell, beside the original.
- **Import.** `store/json.rs` `import` stops rejecting an unknown type (`json.rs:189`): it keeps the string and the
  generic fields it recognises and ignores the rest. For every kind, `import` keeps only `kind.fields()` plus the group
  (plan D5). A missing, non-string or empty `type` stays an error; only a present, unrecognised string is kept.
- **Fresh-start check.** `restore_session`'s all-jars test for a fresh profile (`session_sync.rs:1486`) now counts a
  lone unknown node as real content, which is right: it is a card the user can see. `JsonNode::kind()` keeps returning `None` for Unknown; the link
  end check is unaffected. Export writes the real string and the fields it has a name for. Import Session
  (`materialize_imported`) carries the string through `Imported`.
- **Import Session** also tears down unknown nodes with the old document.

## Touch points (survey 2026-10-14, plus the review)

- `store/model.rs` (`is_live`, `Kind::fields()` generic list, `Node::ty`, `effective_group`, `with_descendants`); `store/json.rs`
  (`node_json`/`node_to_json` panic for Unknown at `:79`, `Imported`, `import`); `store/ops.rs` (`OpBody::create`).
- `session_sync.rs`: `snapshot`, `spawn_snapshot` (287), `materialize` (503), `RemoteChange`/`RemoteNode`/`Carried`,
  `apply_remote` (746), `restore_session`'s local tuple (1464-1501), `insert_materialized` (715), `fields_of` (70),
  `create_ops` (136-171), `imported_fields`, `materialize_imported`, `runtime_has_type` (95-104: accept the sentinel),
  `check_graph_matches_document`, `teardown_running_session`, `update_live_node` (575-580: add
  Unknown to the no-op arm). `closed.rs:164/182/200` (`node_json` callers). `store/ops.rs:205` (test helper calling
  `OpBody::create`). `fetched.rs` `place_node`. `popups.rs:728-732`.
- Exhaustive `Runtime` matches: `node.rs` (84, 103, 118, 409), `card.rs` (901, 930, 937 plus the stale "no unknown-type
  card yet" comments), `reveal.rs:49`, `session_sync.rs:103`. `graph_nav.rs:293` is a `_` match and `:660` a bench
  closure; check, no change expected.
- Tests that flip: `card_kind.rs:181-190` (asserts `*/*` not restorable, layer 0) and the `kinds()` fixture and
  layer-order test (`:107`, `:194`); `closed.rs:536` (`worth_reopening("application/x-future")`); `model.rs:693`;
  `store/journal/tests.rs:313` (`!is_live`); `store/json.rs:426`.
- `caps/rows.rs`, `caps/edges.rs`, `card_kind.rs`, `closed.rs` (`worth_reopening`, `subtree_entry`, 158/171),
  `links.rs:116`.
- `named_by_name` (`session_sync.rs:955`): unchanged (decision 6, plan D4).
- Gate: `debug_control.rs` EXPORTSTATE adds `stored_type` beside `type`, and `CLOSENODE` replaces `CLOSEGROUP`; `state.rs` `cards()` now counts an unknown card (no existing seed has
  one); a new stage `unknown-type` seeds one and runs the verbs below.

## Testing

- Store: an unknown node is live; a typeless placeholder is not; a `Set` of `NAME` or `POS` on it leaves an extra raw
  field byte-identical; an unknown-typed parent is not an effective group (its member sits in the nearest container).
- Sync: the card appears from a restore and from a remote create; moving it writes one `Set` and a newer client's
  field survives; a remote delete removes it; a typeless placeholder gets its card when `TYPE` arrives.
- Copy: Duplicate, Close then Reopen and a JSON round trip keep the type string and the generic fields and drop a raw
  extra, including an `Id`-typed one; the persisted `closed` entry holds the real type string.
- Caps and edges: link and `contains` accept the sentinel; `opened-from` and `jar` refuse it; a group is still not an
  `opened-from` source and a jar still not a target; a raw unknown type and the sentinel resolve to the same row;
  `TYPE_DOWNLOAD` is still not restorable.
- Gate stage `unknown-type` (seeded node: move, rename, group, close, reopen, duplicate, link), then
  `gate all LOG full`.

## Decided and open

- Decided: live, generic fields only, shell copy, one canonical type (this document).
- Parked: verbatim passthrough of raw fields on a copy; a body that shows the node's raw fields.
- Open: whether `opened-from` should accept it as a source (no; revisit if a newer client's page-made kind needs it).
