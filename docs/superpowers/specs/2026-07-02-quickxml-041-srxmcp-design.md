# quick-xml 0.41 in rust-srxmcp-core + sibling bumps

**Issue:** #103 — Security: upgrade quick-xml to ≥0.41.0 (RUSTSEC-2026-0194 / -0195)
**Date:** 2026-07-02
**Status:** Approved design (final leg of the cascade)

## Problem

CI's `audit` job is red on `main`: `quick-xml < 0.41` is affected by **RUSTSEC-2026-0194** (quadratic run time on duplicate attribute names) and **RUSTSEC-2026-0195** (unbounded namespace-declaration allocation → memory-exhaustion DoS). Fixed in **quick-xml 0.41.0**.

Two in-tree versions:
- `0.37.5` — transitive via sibling crates. **Already fixed and published**: `rustnetconf 0.12.3` and `rustez 0.12.1` now depend on quick-xml 0.41 (issues rustnetconf#31, rustEZ#24, both closed).
- `0.36.2` — **direct** dependency of `rust-srxmcp-core`. This spec covers the remaining in-repo work.

## Decision & key research finding

Full per-crate report: `.superpowers/sdd/quickxml-migration-research.md`. The only breaking change across 0.36→0.41 that touches our code is **quick-xml 0.38.0**: `BytesText::unescape()` was removed (`decode()` does encoding only, not entity resolution) and entity refs now stream as a separate `Event::GeneralRef` instead of being folded into `Text`. `rust-srxmcp-core` never calls `unescape`, so it compiles essentially unchanged — but `GeneralRef` changes reader behavior in one security-sensitive place (`redact.rs`).

## Components

### 1. Dependency bumps

- `Cargo.toml:23` (workspace): `rustez = "0.12.0"` → `"0.12.1"`.
- `rust-srxmcp-core/Cargo.toml:18`: `quick-xml = "0.36"` → `"0.41"`.
- `cargo update` — pulls `rustnetconf 0.12.3` transitively (via rustez 0.12.1).
- **Acceptance:** `Cargo.lock` contains no `quick-xml` version `< 0.41.0`; `rustnetconf ≥ 0.12.3`, `rustez = 0.12.1`.

### 2. `redact.rs` — GeneralRef handling (the one real code change)

`rust-srxmcp-core/src/workflows/support_bundle/redact.rs`, `redact_xml()` reader loop (currently ~lines 74-112). Today:
- `Event::Text(_) | Event::CData(_)` under `redact_depth > 0` → write `REDACTED_MARKER`.
- catch-all `Ok(event)` → `write_event(event)` verbatim (this is where `GeneralRef` currently lands).

**Security gap:** under quick-xml 0.41, an entity inside a redacted secret (`foo&amp;bar`) streams as `Text("foo")`, `GeneralRef("amp")`, `Text("bar")`. The `GeneralRef` falls through the catch-all → **written verbatim** → a fragment of the "redacted" element is emitted.

**Fix:** extend the redaction guard to cover `Event::GeneralRef` so entity refs under `redact_depth > 0` are suppressed. To avoid a split value collapsing into multiple repeated markers, add a small `emitted_marker_run: bool` flag: the first Text/CData/GeneralRef of a redacted run emits one `REDACTED_MARKER`; subsequent redacted text-ish events in the same contiguous run emit nothing; the flag resets on any `Start`/`End` (i.e. a new element boundary). Net: one `[REDACTED]` per redacted text run regardless of entity splitting, and **no** verbatim fragment.

**Non-redacted path:** `GeneralRef` must continue to pass through the catch-all and round-trip correctly (`write_event(Event::GeneralRef(...))` must re-emit `&name;`) so non-secret XML containing entities is not corrupted. The plan must verify this round-trips; if `write_event` does not faithfully re-emit `GeneralRef`, reconstruct the reference explicitly (`&<name>;`) on the non-redacted path.

### 3. `xml.rs`

Write-only (`Writer`, `BytesStart/BytesEnd/BytesText::new`, `push_attribute`). Expected **zero changes** — verify it compiles under 0.41; `BytesText::new` still auto-escapes on write.

## Testing

New unit tests in `redact.rs` (alongside the existing redaction tests):
- **Redacted + entity:** a redacted element (one of `REDACT_ELEMENT_NAMES`) whose text is `abc&amp;def` (and one with `&lt;`) → output contains exactly one `REDACTED_MARKER` for that element and **no** `amp`/`&`/`def` fragment leaked.
- **Redacted + pure entity:** a redacted element whose text is only `&amp;` → output shows `REDACTED_MARKER`, no leak.
- **Non-redacted round-trip:** a non-redacted element with text `a &amp; b` → output preserves the entity (`&amp;`), value intact.
- Existing redaction tests (#85/#89/#91/#92 lineage) still pass unchanged.

Workspace gates: `cargo build --workspace`, `cargo test --workspace` (0 failures), `cargo fmt -- --check`, `cargo clippy`. **Acceptance gate:** `cargo audit` reports **no RUSTSEC-2026-0194/-0195** (and RUSTSEC-2026-0189 stays absent). CI `audit` job goes green.

## Deploy

Rebuild `rust-srxmcp` release, deploy to ct601 (pve2) via the standard procedure (backup+rotate, stop, `pct push`, chown, start; note ct601 is on **pve2**). Live smoke `:30032`: `tools/list` (9 tools), one read-only call (e.g. `srxmcp_status`), and — since redaction changed — a `collect_jtac_support_bundle` redaction sanity check if a target device is available, else rely on the unit tests. Confirm the Host allowlist still works (bogus Host → 403) since the srx binary is being replaced.

## Risks

1. **GeneralRef round-trip on the non-redacted path** — a wrong re-emit corrupts bundle XML for any element containing `&`/`<`/`>`. Pinned by the round-trip test.
2. **Redaction suppression** — must not leak a fragment nor drop the marker entirely. Pinned by the redacted+entity and pure-entity tests.
3. Transitive resolution: confirm `cargo update` fully removes quick-xml `<0.41` (both 0.36.2 and 0.37.5) — a lingering old version keeps the advisory.

## Out of scope

- No behavioral change to `xml.rs` or the redaction element list.
- The `anyhow` RUSTSEC-2026-0190 warning + yanked `aes` (via russh) remain out of scope (warnings, non-blocking).
- rustnetconf/rustez internal migration — already done and published.
