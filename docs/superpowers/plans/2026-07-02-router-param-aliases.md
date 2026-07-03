# Router-param aliases Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every tool's router-target parameter accept `router` / `router_name` (and `routers` + single-string for batch) via additive serde aliases, so a caller's first guess never fails with an opaque `missing field` error (#104).

**Architecture:** Additive `#[serde(alias = …)]` on each router-target field in both crates (no renames, backward-compatible), plus a `string_or_vec` deserializer so the batch `routers: Vec<String>` also accepts a single string. Test-driven via serde round-trip tests.

**Tech Stack:** Rust, serde, schemars.

## Global Constraints

- Additive only — NO field renames; the existing primary names still work.
- Accepted set: single-router fields accept `router` **and** `router_name`; `ExecuteBatchArgs.routers` accepts `router`, `router_name`, `routers`, and a JSON string (→ one-element vec) or array.
- No change to tool behavior or the tool surface.
- `cargo test --workspace` 0 failures; `cargo fmt -- --check` + `cargo clippy --workspace` clean.

---

### Task 1: junos aliases + batch string-or-list (`tools/mod.rs`)

**Files:**
- Modify: `rust-junosmcp-core/src/tools/mod.rs`

**Interfaces:**
- Produces: `fn string_or_vec<'de, D>(d: D) -> Result<Vec<String>, D::Error>` (private helper in mod.rs).

- [ ] **Step 1: Write the failing round-trip tests**

Add to the `#[cfg(test)] mod tests` block in `rust-junosmcp-core/src/tools/mod.rs`:

```rust
#[test]
fn router_alias_accepts_router_and_router_name() {
    // Single-router tool: both names deserialize to the same field.
    let a: ExecuteCommandArgs = serde_json::from_value(
        serde_json::json!({"router":"r1","command":"show version"})).unwrap();
    assert_eq!(a.router_name, "r1");
    let b: ExecuteCommandArgs = serde_json::from_value(
        serde_json::json!({"router_name":"r1","command":"show version"})).unwrap();
    assert_eq!(b.router_name, "r1");
}

#[test]
fn get_config_and_upgrade_accept_router_alias() {
    let g: GetConfigArgs = serde_json::from_value(serde_json::json!({"router":"r1"})).unwrap();
    assert_eq!(g.router_name, "r1");
    let u: UpgradeJunosArgs = serde_json::from_value(serde_json::json!({
        "router":"r1","source_path":"x.tgz","target_version":"25.4R1.12"})).unwrap();
    assert_eq!(u.router_name, "r1");
}

#[test]
fn batch_accepts_list_string_and_aliases() {
    let list: ExecuteBatchArgs = serde_json::from_value(serde_json::json!({
        "routers":["a","b"],"commands":["show version"]})).unwrap();
    assert_eq!(list.routers, vec!["a".to_string(), "b".to_string()]);

    let one: ExecuteBatchArgs = serde_json::from_value(serde_json::json!({
        "routers":"a","commands":["show version"]})).unwrap();
    assert_eq!(one.routers, vec!["a".to_string()]);

    let via_router: ExecuteBatchArgs = serde_json::from_value(serde_json::json!({
        "router":"a","commands":["show version"]})).unwrap();
    assert_eq!(via_router.routers, vec!["a".to_string()]);

    let via_router_name: ExecuteBatchArgs = serde_json::from_value(serde_json::json!({
        "router_name":["a","b"],"commands":["show version"]})).unwrap();
    assert_eq!(via_router_name.routers, vec!["a".to_string(), "b".to_string()]);
}
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test -p rust-junosmcp-core tools::tests::router_alias_accepts_router_and_router_name tools::tests::batch_accepts_list_string_and_aliases 2>&1 | tail -15`
Expected: FAIL — `{"router":…}` currently errors (`missing field router_name` / `routers`); the batch string form fails to deserialize into a `Vec`.

- [ ] **Step 3: Add the `string_or_vec` helper**

Near the top of `rust-junosmcp-core/src/tools/mod.rs` (after the existing `use` lines / default fns), add:

```rust
/// Deserialize a `Vec<String>` from either a JSON string (→ one-element vec)
/// or a JSON array of strings. Lets the batch `routers` field accept a single
/// router name as well as a list.
fn string_or_vec<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}
```

- [ ] **Step 4: Add `#[serde(alias = "router")]` to every single-router field**

In `rust-junosmcp-core/src/tools/mod.rs`, add `#[serde(alias = "router")]` on the line immediately above `pub router_name: …` in each of these structs: `ExecuteCommandArgs`, `GetConfigArgs`, `ConfigDiffArgs`, `GatherFactsArgs`, `LoadCommitArgs`, `CommitCheckArgs`, `ExecutePfeArgs`, `TemplateArgs` (`Option<String>`), `TransferFileArgs`, `FetchFileArgs`, `ListStagedFilesArgs` (`Option<String>`), `UpgradeJunosArgs`. Example (ExecuteCommandArgs):

```rust
    /// The name of the router.
    #[serde(alias = "router")]
    pub router_name: String,
```

(For the two `Option<String>` fields, the `#[serde(default)]` already present stays; add the `alias` attribute alongside — multiple serde attrs can share one `#[serde(...)]` or stack; e.g. `#[serde(default, alias = "router")]`.)

- [ ] **Step 5: Update `ExecuteBatchArgs.routers`**

In `ExecuteBatchArgs`, change the `routers` field to:

```rust
    /// Routers to execute against. Must be non-empty. Accepts a list, or a
    /// single router name; the keys `router` / `router_name` are also accepted.
    #[serde(alias = "router", alias = "router_name", deserialize_with = "string_or_vec")]
    pub routers: Vec<String>,
```

- [ ] **Step 6: Build (watch for a schemars/deserialize_with conflict)**

Run: `cargo build -p rust-junosmcp-core 2>&1 | tail -15`
Expected: compiles. **If** schemars errors on the `routers` field because of `deserialize_with`, add `#[schemars(with = "Vec<String>")]` to that field and rebuild. Do NOT change the field's declared type.

- [ ] **Step 7: Run tests — verify pass**

Run: `cargo test -p rust-junosmcp-core tools:: 2>&1 | tail -12`
Expected: the 3 new alias tests PASS; all pre-existing `mod.rs` arg tests still PASS (aliases are additive; primary names unchanged).

- [ ] **Step 8: fmt + clippy + commit**

Run: `cargo fmt && cargo fmt -- --check && cargo clippy -p rust-junosmcp-core 2>&1 | tail -3`

```bash
git add rust-junosmcp-core/src/tools/mod.rs
git commit -m "feat(core): accept router/router_name aliases; batch accepts string-or-list (#104)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019mPwHV2n6YmBTd5j8HcAAJ"
```

---

### Task 2: srx aliases (`rust-srxmcp-core/src/workflows/*`)

**Files:**
- Modify: every `rust-srxmcp-core/src/workflows/*.rs` file with a `pub router: String` arg field.

**Interfaces:**
- Consumes: nothing from Task 1 (separate crate).

- [ ] **Step 1: Enumerate the fields**

Run: `grep -rn "pub router:" rust-srxmcp-core/src/workflows/`
This lists every arg struct's `router` field (expected in: `support_bundle/mod.rs`, `services_status.rs`, `cluster_health.rs`, `cluster_status.rs`, `vpn_lifecycle.rs`, `license.rs`, `signature_package/plan.rs`, `idp_package.rs`, `appid_package.rs`). Use the actual grep output as the authoritative list.

- [ ] **Step 2: Write a failing round-trip test**

Pick one srx arg struct that is `pub` and deserializable — e.g. in `rust-srxmcp-core/src/workflows/services_status.rs`, add to its test module (or create one):

```rust
#[test]
fn router_name_alias_resolves() {
    // `router` is primary; `router_name` must also deserialize.
    let a: ServicesStatusArgs = serde_json::from_value(
        serde_json::json!({"router":"r1"})).unwrap();
    assert_eq!(a.router, "r1");
    let b: ServicesStatusArgs = serde_json::from_value(
        serde_json::json!({"router_name":"r1"})).unwrap();
    assert_eq!(b.router, "r1");
}
```

(Adjust the struct name and any other required fields to what `ServicesStatusArgs` actually needs — read the struct first. If `ServicesStatusArgs` has other required fields, include them in the json.)

- [ ] **Step 3: Run — verify failure**

Run: `cargo test -p rust-srxmcp-core router_name_alias_resolves 2>&1 | tail -10`
Expected: FAIL — `{"router_name":"r1"}` errors with `missing field router` before the alias is added.

- [ ] **Step 4: Add `#[serde(alias = "router_name")]` to every `router` field**

For each `pub router: String` (or `Option<String>`) found in Step 1, add `#[serde(alias = "router_name")]` on the line immediately above it. If the field already has a `#[serde(...)]` attribute, merge (e.g. `#[serde(default, alias = "router_name")]`). Example:

```rust
    /// Target router (device name in the inventory).
    #[serde(alias = "router_name")]
    pub router: String,
```

- [ ] **Step 5: Run tests — verify pass**

Run: `cargo test -p rust-srxmcp-core 2>&1 | tail -8`
Expected: the new alias test PASSES; all existing srx tests still pass.

- [ ] **Step 6: fmt + clippy + full workspace + commit**

Run: `cargo fmt && cargo fmt -- --check && cargo clippy --workspace --all-targets 2>&1 | tail -3 && cargo test --workspace 2>&1 | grep -E "FAILED|error\[" || echo "workspace clean"`
Expected: clean; 0 workspace failures.

```bash
git add rust-srxmcp-core/src/workflows/
git commit -m "feat(srxmcp): accept router_name alias on router param (#104)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_019mPwHV2n6YmBTd5j8HcAAJ"
```

---

## Self-Review

**Spec coverage:**
- 12 junos single-router fields gain `alias = "router"` → Task 1 Step 4. ✔
- Batch `routers` gains `router`/`router_name` aliases + string-or-list → Task 1 Steps 3, 5. ✔
- srx `router` fields gain `alias = "router_name"` → Task 2 Step 4. ✔
- schemars/deserialize_with fallback → Task 1 Step 6. ✔
- Round-trip tests both crates; existing tests still pass → Task 1 Steps 1/7, Task 2 Steps 2/5. ✔
- No renames / additive → the plan only adds attributes. ✔

**Placeholder scan:** No TBD/TODO. Task 2 Step 1 uses a grep to enumerate (the authoritative list) rather than hardcoding line numbers that may drift — with the expected file list stated; Step 2 notes to read the struct for its other required fields. All code steps show code.

**Type consistency:** `string_or_vec` signature and `routers: Vec<String>` consistent (Task 1). Single-router fields keep type `String`/`Option<String>`; only an attribute is added. srx `router: String` unchanged in type.

**Risk note for implementer:** (1) When a field already has `#[serde(default)]` (the two `Option` junos fields), MERGE into one attribute (`#[serde(default, alias = "router")]`) rather than stacking two conflicting `#[serde]` for `default` — both forms compile, but keep it clean. (2) The `#[serde(untagged)]` `OneOrMany` in `string_or_vec` tries `String` first then `Vec` — correct for our inputs; a JSON number would fail both (fine, it's an error). (3) In srx, only add the alias to structs whose `router` field means the target device (all found do); don't touch unrelated `router` identifiers.
