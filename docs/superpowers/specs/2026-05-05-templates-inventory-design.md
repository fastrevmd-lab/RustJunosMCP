# Templates + Inventory Mutation (sub-project #4) — Design

**Status:** Draft, 2026-05-05
**Target release:** v0.2.2
**Sub-project:** #4 of the v0.x roadmap (closes upstream parity gap)
**Predecessors:** v0.2.0 (sub-project #2, remote transport + auth, PR #4), v0.2.1 (sub-project #3, PFE + batch, PR #5)

## 1. Goal

Reach **full tool-surface parity** with `Juniper/junos-mcp-server` by adding the three remaining tools:

1. `render_and_apply_j2_template` — render a Jinja2 template and optionally commit the rendered config to one or more routers.
2. `add_device` — dynamically add a Junos device to the in-memory inventory and persist to `devices.json`.
3. `reload_devices` — re-read `devices.json` (current path) or swap to a new path.

Plus one minor CI follow-up from sub-project #3: switch the `pfe_smoke` connect-failure target from `203.0.113.1:1` (TEST-NET-3, ~140 s TCP-connect timeout) to `127.0.0.1:1` (instant `ECONNREFUSED`).

After v0.2.2, `KNOWN_TOOLS` carries 11 entries, matching the upstream Python implementation exactly.

## 2. Non-goals

- **Token-store linkage.** `add_device` does not modify the token store. If an existing token has `--routers 'edge-*'` and the operator adds `core-3`, the new router remains invisible to that token until it is rotated. Documented as a sharp edge in README.
- **Template file libraries on disk.** `template_content` is always inline (parity with upstream). A future sub-project may add `--template-dir` with on-disk allowlisting; not in scope here.
- **Git-backed templates.** Out of scope.
- **Per-router scope on `add_device`/`reload_devices`.** These tools do not address one device — they reshape the inventory itself. Tool-scope (token allowlist) is the only gate.
- **Elicitation polish for streamable-http.** The full elicitation path is implemented for stdio clients that advertise it. Streamable-http callers (Claude Desktop today, possibly more tomorrow) get the args-fallback behavior. A future polish pass can revisit if/when client coverage broadens.

## 3. Release plan: one spec, two PRs

This single spec drives **two** implementation PRs, each shippable on its own:

### PR #6 — Templates (`feature/templates-inventory`, then split for review)

- `render_and_apply_j2_template` tool
- `minijinja` + `serde_yml` dependencies
- Render path through the existing commit infrastructure (reuses `load_and_commit_config`'s blocklist + format checks)
- The trivial `pfe_smoke` CI fix (`203.0.113.1:1` → `127.0.0.1:1`)
- Tool count assertion: 8 → 9 (`stdio_smoke::lists_nine_tools`)

### PR #7 — Inventory mutation (`feature/inventory-mutation`)

- `add_device` tool (rmcp elicitation + args fallback)
- `reload_devices` tool (optional `file_name`)
- `Inventory` mutability refactor (`Arc<Inventory>` → `Arc<ArcSwap<Inventory>>` in `DeviceManager`)
- Atomic `devices.json` write (tempfile + rename, fsync)
- New CLI flags: `--inventory-readonly`, `--allow-password-auth-add`
- SIGHUP extended to also re-read inventory
- Tool count assertion: 9 → 11 (`stdio_smoke::lists_eleven_tools`)
- Blocking on PR #6 only via `KNOWN_TOOLS` count assertion. PR #7 rebases on main once #6 lands.

PR ordering is a convenience; the work is logically separable. If priorities shift, either PR can ship first by adjusting the smoke-test count assertions.

## 4. Tool surface

### 4.1 `render_and_apply_j2_template`

Argument schema (parity with upstream):

```jsonc
{
  "template_content": "<jinja2 template string>",   // required
  "vars_content":     "<JSON or YAML object>",       // required
  "router_name":      "core-1",                      // optional, exclusive with router_names
  "router_names":     ["edge-1", "edge-2"],          // optional, exclusive with router_name
  "apply_config":     false,                         // default false
  "commit_comment":   "ack-1234",                    // optional
  "dry_run":          false,                         // default false
  "config_format":    "set"                          // optional override; auto-detected otherwise
}
```

**Vars sniff:** `vars_content.trim_start()` first non-whitespace `{` → `serde_json::from_str`; otherwise `serde_yml::from_str`. Both produce `serde_json::Value`. Reject anything that isn't a top-level object (`Value::Object`).

**Render:** `minijinja::Environment::new()` with `undefined_behavior(UndefinedBehavior::Strict)` and no autoescape (config text is not HTML). Strict-undefined fails loudly with the missing variable name rather than silently emitting empty strings into router config.

**Format auto-detect** (when `config_format` is omitted):
- First non-whitespace char `<` → `xml`
- Any line starting with `set ` or `delete ` → `set`
- Otherwise → `text`

**Apply path** (when `apply_config = true`): the rendered string is passed through the same code path as `load_and_commit_config`:

1. Pre-flight router scope (per-router, first-failure short-circuit).
2. Pre-flight blocklist on the **rendered** payload — config-rule subset of `_blocklist_defaults` and per-device `blocklist`. Same as `load_and_commit_config` today.
3. If any device has effective config rules, `config_format` MUST be `set`. `text` and `xml` are rejected pre-flight.
4. Per-router commit: `dry_run = true` returns the diff via the existing `junos_config_diff` semantics; otherwise commit with optional `commit_comment`.

**Result shape** (parallel to `execute_junos_command_batch`): one row per router with `rendered_template`, `diff` (if dry-run), `commit_id` (if applied), or `error`. `commands.len()` invariant analogue: `rows.len() == routers.len()`.

**Failure modes:**
- `JmcpError::TemplateSyntax(String)` — minijinja parse error with line/col
- `JmcpError::TemplateVars(String)` — vars parser failure (mentions which parser was tried)
- `JmcpError::TemplateRender(String)` — strict-undefined or runtime render error
- `JmcpError::TemplateFormatMismatch` — `text`/`xml` payload against a device with config rules

### 4.2 `add_device`

Argument schema (all optional at the schema level — elicitation may fill gaps):

```jsonc
{
  "device_name":     "core-3",
  "device_ip":       "10.0.0.3",
  "device_port":     22,
  "username":        "automation",
  "auth": {
    "type": "ssh_key",
    "private_key_path": "/etc/jmcp/keys/id_ed25519"
  }
  // OR auth.type = "password" with --allow-password-auth-add
}
```

**Validation gates** (pre-flight, in order):

1. `--inventory-readonly` set → `JmcpError::InventoryReadonly`.
2. `device_name` already exists → `JmcpError::DeviceExists(name)`.
3. `auth.type == "password"` and `--allow-password-auth-add` not set → `JmcpError::PasswordAuthDisabled`.
4. `device_name` matches `^[A-Za-z0-9_.-]+$` (rules out shell metas / globs / spaces).
5. `device_ip` parses as `IpAddr` OR is a valid hostname (RFC 1123 letters-digits-hyphens, ≤253 chars).
6. `device_port` ∈ `1..=65535` (default 22 if omitted).

**Elicitation flow:**
- Tool struct uses `Option<…>` for every required field.
- On entry, inspect the rmcp 0.8.5 client capabilities. If `elicitation` is advertised, `peer.elicit(...)` with a typed schema for missing required fields. Default UX for stdio.
- If elicitation is not advertised (typical streamable-http today), the args-fallback path runs: any missing required field → `JmcpError::MissingArguments(Vec<String>)` listing exactly what is needed.
- Fallback is not a degraded experience — it is the documented contract for non-elicitation transports.

**Write path:**
1. Acquire `inventory_write_lock` (single `tokio::sync::Mutex<()>`).
2. Re-read `devices.json` from disk; SHA-256-hash and compare against the last-known-content hash stored alongside the `ArcSwap`. Mismatch → `JmcpError::InventoryDriftedOnDisk`.
3. Insert the new device into the in-memory map (`IndexMap` — appends at end, preserving order of existing entries).
4. Serialize the whole inventory to a temp file (`<path>.tmp.<pid>.<rand>`) in the same parent directory. Whole-file round-trip preserves `_blocklist_defaults`, per-device `blocklist`, and any other top-level keys not modeled by `Inventory` (round-trip via `serde_json::Value` first, then merge in the new device).
5. `fsync` the temp file, then `rename` over `devices.json`. Same-filesystem rename is atomic on Linux (POSIX `rename(2)` guarantees).
6. `ArcSwap::store` the new `Arc<Inventory>`. Update last-known-content hash.

**Failure modes:**
- `JmcpError::InventoryReadonly`
- `JmcpError::DeviceExists(String)`
- `JmcpError::PasswordAuthDisabled`
- `JmcpError::InvalidDeviceName(String)` / `InvalidDeviceIp` / `InvalidDevicePort(u32)`
- `JmcpError::MissingArguments(Vec<String>)` — fallback path only
- `JmcpError::InventoryDriftedOnDisk` — TOCTOU guard
- `JmcpError::InventoryWrite(io::Error)` — disk full, permission denied, ENOSPC

### 4.3 `reload_devices`

Argument schema:

```jsonc
{ "file_name": "/etc/jmcp/devices-staging.json" }   // optional
```

**Semantics:**
- Omitted / `null` / `""` → re-read the current `--device-mapping` path.
- Provided → switch to the new path: parse, validate, swap both `inventory` and `inventory_path` atomically (two consecutive `ArcSwap::store` calls; readers are still safe).

**Validation gates:**
1. `--inventory-readonly` set → `JmcpError::InventoryReadonly`.
2. Path must exist, be a regular file, and be readable.
3. Parse must succeed (full inventory schema, including `_blocklist_defaults`).
4. Empty inventory rejected (`devices: {}` likely indicates a mistake) → `JmcpError::EmptyInventory`.

**Write path:** none. `reload_devices` is read-only on disk; only `ArcSwap` state mutates.

**SIGHUP integration:** the existing SIGHUP handler already re-reads tokens. Extended in PR #7 to ALSO re-read the *current* inventory path (calls into the same code path as `reload_devices` with `file_name = None`). Single signal, both stores. No SIGUSR1.

**Result shape:**

```jsonc
{
  "previous_router_count": 7,
  "new_router_count": 8,
  "added": ["core-3"],
  "removed": [],
  "changed": [],
  "inventory_path": "/etc/jmcp/devices.json"
}
```

`changed` is the subset of names present in both old and new inventories whose `(ip, port, username, auth)` tuple differs. Used by clients to surface inventory drift.

**Failure modes:**
- `JmcpError::InventoryReadonly`
- `JmcpError::InventoryRead(io::Error)` — file missing, permission denied
- `JmcpError::InventoryParse(String)` — serde error
- `JmcpError::EmptyInventory`

## 5. Architecture

### 5.1 Inventory mutability

Today (v0.2.1):

```rust
pub struct DeviceManager {
    inventory: Arc<Inventory>,
    // …
}
```

After PR #7:

```rust
pub struct DeviceManager {
    inventory: Arc<ArcSwap<Inventory>>,
    inventory_path: Arc<ArcSwap<PathBuf>>,
    inventory_hash: Arc<ArcSwap<[u8; 32]>>,        // SHA-256 of last-known on-disk content
    inventory_write_lock: Arc<tokio::sync::Mutex<()>>,
    // …
}
```

**Read sites** call `.load()` once at handler entry, then operate on the snapshot for the entire call. This gives snapshot semantics: a tool that takes 30 seconds of device I/O won't see mid-flight `add_device` results, and a concurrent `add_device` won't observe a partial read.

**Write sites** (`add_device` only — `reload_devices` doesn't write to disk) acquire `inventory_write_lock` for the full read-current → mutate → atomic-write → swap sequence. Readers never block on writers; only concurrent writers serialize.

**Atomicity guarantee:** any reader sees either the pre-write or post-write inventory, never a partial. This is enforced by `ArcSwap::store` semantics; the on-disk side is enforced by tempfile + rename.

### 5.2 Atomic file write

Implemented in `rust-junosmcp-core/src/inventory.rs` as a free function `write_inventory_atomic(path: &Path, inv: &serde_json::Value) -> io::Result<()>`:

1. Resolve `parent = path.parent()`.
2. Verify parent is writable (`metadata().permissions()` check; clear error if not).
3. Create temp file: `<parent>/<filename>.tmp.<pid>.<rand>` via `tempfile::NamedTempFile::new_in(parent)`.
4. Serialize `inv` to the temp file (pretty-printed, two-space indent).
5. `temp.as_file().sync_all()` — fsync.
6. `temp.persist(path)` — atomic rename. Errors if cross-FS (callers should ensure parent is on the same FS).
7. Optionally fsync the parent directory on Linux for durability under crash. Out of scope for v0.2.2 (tested rename ordering is sufficient for the threat model — operator-driven changes, not crash-safety-critical paths).

The temp file is created via `tempfile` crate (already in workspace deps for tests; promote to runtime dep). Permissions on the rendered file match the source: read existing mode bits via `fs::metadata(path)`, apply with `set_permissions` after rename.

### 5.3 TOCTOU defence

A `add_device` call could race against:
- An operator hand-editing `devices.json`
- Another `add_device` on a different connection
- Config-management replacing the file (Ansible, Salt, etc.)

The `inventory_write_lock` serializes our internal writers. The `inventory_hash` check catches all other writers: if the on-disk SHA-256 at the start of `add_device` doesn't match what we stored on the last load/write, we reject the call with `JmcpError::InventoryDriftedOnDisk` and tell the caller to retry after `reload_devices`.

This is a deliberate "fail loud" choice. Auto-merging external edits is hard and dangerous; a clear retry contract is safer.

### 5.4 Round-trip preservation

`Inventory` does not model every field of `devices.json` — `_blocklist_defaults`, per-device `blocklist`, and any operator-specific extension fields would be silently dropped if we round-tripped through the typed struct. `add_device` therefore operates on a `serde_json::Value` representation of the file:

1. Read file → `serde_json::Value`.
2. Locate the `devices` map (or top-level if devices live at root — match upstream's structure).
3. Insert the new entry at the end (`Value::Object` is `IndexMap`-backed in `serde_json` ≥ 1.0.66 with the `preserve_order` feature).
4. Serialize back.

Enable `serde_json/preserve_order` feature explicitly; verify in tests that key order is preserved.

### 5.5 Concurrency invariants

| Scenario | Behavior |
|---|---|
| Two `add_device` calls in flight | Serialized by `inventory_write_lock`. Second call sees first's result on its hash check. |
| `add_device` mid-flight, concurrent `execute_junos_command` | Read snapshot is whatever was loaded at command entry. New device may or may not be visible — depends on timing; both states are valid. |
| `reload_devices` mid-flight, concurrent `add_device` | Lock-ordered: `add_device` either runs first (its result is visible to reload's hash check) or second (its hash check fails because reload changed the inventory in-memory but not on disk — this is a bug; mitigation below). |
| External edit during `add_device` | Hash check fails, `JmcpError::InventoryDriftedOnDisk`. |
| Crash mid-write | Atomic rename: either pre-write or post-write file. No partial. |

**Reload-vs-add ordering:** `reload_devices` also acquires `inventory_write_lock` (read-only on disk, but mutually exclusive with `add_device`). This makes the locking model trivial: any inventory mutation, on-disk or in-memory, is single-threaded. Reload-after-add observes the persisted device; add-after-reload starts from the freshly-loaded inventory and the hash check sees the file reload just read.

## 6. Dependencies

PR #6 adds:
- `minijinja = "2"` — Jinja2-fidelity template engine. Active maintenance, Mozilla-style serde integration.
- `serde_yml = "0.0.12"` — YAML parser. Active fork of `serde_yaml` (which is deprecated). Adds ~3 transitive crates.

PR #7 adds:
- `arc-swap` — already in workspace deps (used by token store).
- `tempfile` — already in workspace deps (used by tests). Promote to runtime dep for atomic write.
- `indexmap` — already a transitive dep via `serde_json/preserve_order`. Enable the feature explicitly.

No new MSRV bumps. All deps support stable Rust 1.75+ (we're at edition 2021).

## 7. CLI

Two new flags, both optional:

```
--inventory-readonly
    Reject add_device and reload_devices unconditionally. Independent of token
    scopes. Useful for hardened deployments where the inventory is managed
    out-of-band (config-mgmt, baked image).

--allow-password-auth-add
    Permit add_device to accept auth.type="password". Off by default.
    Mutually exclusive with --inventory-readonly.
```

Both apply to both transports (stdio + streamable-http).

## 8. Auth / token integration

`KNOWN_TOOLS` extended by 3:

```rust
const KNOWN_TOOLS: &[&str] = &[
    "get_router_list",
    "gather_device_facts",
    "execute_junos_command",
    "get_junos_config",
    "junos_config_diff",
    "load_and_commit_config",
    "execute_junos_pfe_command",
    "execute_junos_command_batch",
    "render_and_apply_j2_template",   // PR #6
    "add_device",                      // PR #7
    "reload_devices",                  // PR #7
];
```

Default token scope is still explicit-allowlist. No token reaches the new tools without explicit operator action via `token add --tools …` or `token rotate`. Tokens minted before sub-project #4 keep working unchanged.

Existing gating order is unchanged for both new tools:

```
transport → AuthLayer → CallerCtx → tool scope → router scope → blocklist
```

For `add_device` and `reload_devices`, the router-scope step is a no-op (these tools don't address one device). Tool-scope is the only auth gate.

For `render_and_apply_j2_template`, router-scope runs once per router in the selector list, with first-failure short-circuit (matches the batch tool's behavior).

**Documented sharp edge** (README + release notes):

> `add_device` does not modify the token store. If a token has `--routers 'edge-*'` and you `add_device` for `core-3`, the existing token will not see the new router. Mint a new token or rotate scopes after `add_device`.

## 9. Tests

### 9.1 Unit tests

`rust-junosmcp-core/src/tools/template.rs`:
- `vars_sniff_routes_json` / `vars_sniff_routes_yaml`
- `vars_sniff_rejects_non_object`
- `render_strict_undefined_fails_with_var_name`
- `render_minijinja_filters_work` (smoke through `default`, `upper`, `length`)
- `format_autodetect_xml` / `_set` / `_text`
- `apply_path_blocklist_rejects_rendered_payload`
- `apply_path_format_mismatch_rejects_text_when_rules_present`

`rust-junosmcp-core/src/inventory.rs` (new mutation module):
- `atomic_write_replaces_file_in_place`
- `atomic_write_preserves_blocklist_defaults`
- `atomic_write_preserves_per_device_blocklist`
- `atomic_write_preserves_key_order`
- `add_device_rejects_name_collision`
- `add_device_rejects_invalid_name_regex`
- `add_device_rejects_password_auth_when_flag_disabled`
- `add_device_rejects_invalid_ip`
- `add_device_rejects_out_of_range_port`
- `reload_devices_swaps_path` / `_rejects_empty_inventory`
- `hash_mismatch_after_external_edit_aborts_add`
- `concurrent_add_serialized_by_write_lock` (tokio test)

`rust-junosmcp/src/cli.rs`:
- `inventory_readonly_and_allow_password_auth_add_are_mutually_exclusive`
- `inventory_readonly_default_off`

### 9.2 Integration smoke tests

`rust-junosmcp/tests/template_smoke.rs`:
- `render_only_path_returns_rendered_string` (apply_config=false)
- `render_with_yaml_vars` / `render_with_json_vars`
- `render_strict_undefined_surfaces_through_tool_call`

`rust-junosmcp/tests/add_device_smoke.rs`:
- `add_then_reload_then_router_list_shows_new_device` (full cycle, `tempfile::TempDir`)
- `add_device_args_fallback_when_no_elicitation_advertised`
- `add_device_inventory_readonly_returns_clear_error`
- `add_device_password_auth_disabled_by_default`

`rust-junosmcp/tests/reload_devices_smoke.rs`:
- `reload_devices_no_args_re_reads_current_path`
- `reload_devices_with_file_name_swaps_inventory`
- `reload_devices_reports_added_removed_changed_diff`
- `reload_devices_empty_inventory_rejected`
- `reload_devices_inventory_readonly_returns_clear_error`

### 9.3 Modified existing tests

- `rust-junosmcp/tests/stdio_smoke.rs::lists_eight_tools` (v0.2.1) → renamed `lists_nine_tools` after PR #6, then `lists_eleven_tools` after PR #7. `EXPECTED_TOOLS` extended both times.
- `rust-junosmcp/tests/pfe_smoke.rs::pfe_connect_failure_surfaces_through_tool_call`: replace `203.0.113.1:1` with `127.0.0.1:1`. Expected runtime drops from ~140 s to <1 s. Bundled into PR #6.

### 9.4 Real-device tests (`#[ignore]`)

`rust-junosmcp-core/tests/integration_real_device.rs`:
- `live_render_show_version_template_dry_run` — render `show version` through the template tool, dry-run only. Verifies parity contract end-to-end.
- `live_add_device_persists_then_reload` — add device to a temp file, reload from that path, assert presence in `get_router_list`. Cleans up temp file. Uses `JMCP_TEST_HOST` etc.

### 9.5 Verification checklist (per PR, Task-17 equivalent)

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check        # CI-blocker per ci_format_check memory
cargo audit
```

## 10. Documentation

### 10.1 README updates

After PR #6:
- Add to "Feature scope" a new subsection "v0.2 follow-up: Templates (released)" listing the `render_and_apply_j2_template` tool, vars-format sniff, and minijinja choice.
- Update top-of-file callout from v0.2.1 → v0.2.2 once both PRs land.

After PR #7:
- Extend the "v0.2 follow-up: Templates" subsection or add a sibling "Inventory mutation" subsection covering `add_device` / `reload_devices`, the new CLI flags, and the documented sharp edge about tokens not auto-updating.
- Update CLI section with the two new flags.
- Add a paragraph about SIGHUP now reloading both tokens AND inventory.

### 10.2 `devices-template.json`

No structural change. Optional: add a comment indicating which top-level fields are preserved by `add_device`'s round-trip.

### 10.3 Release notes

v0.2.2 release notes call out:
- 3 new tools (full upstream parity)
- `--inventory-readonly` and `--allow-password-auth-add` flags
- SIGHUP now also reloads inventory
- The token-scope sharp edge

## 11. Migration / compat

- Inventories without `add_device` use are unchanged; round-trip preserves all unknown top-level fields.
- Existing tokens unchanged. New tools require explicit `token add --tools render_and_apply_j2_template,add_device,reload_devices` (or rotate).
- `AuthConfig` enum unchanged on disk. `add_device` writes the same JSON shape.
- Drop-in compat with upstream Juniper inventory remains intact.

## 12. Open follow-ups (deferred to sub-project #5+)

- Template files on disk (`--template-dir`, allowlisted paths)
- Git-backed templates
- Auto-update token-store router scopes when `add_device` runs (would require a clear policy choice)
- Streamable-http client elicitation (depends on client capabilities maturing)
- `remove_device` tool (the upstream Python implementation does not have one; out of scope for parity)

## 13. References

- Upstream: <https://github.com/Juniper/junos-mcp-server>
- Upstream tools list verified 2026-05-05 against `jmcp.py` `TOOL_HANDLERS`.
- Plan for sub-project #3 (PFE + batch): `docs/superpowers/plans/2026-05-05-pfe-batch.md`
- Spec for sub-project #2 (remote transport + auth): `docs/superpowers/specs/2026-05-05-remote-transport-auth-design.md`
- CI rustfmt enforcement memory: `~/.claude/projects/-home-mharman-RustJunosMCP/memory/ci_format_check.md`
- `pfe_smoke` slow-test follow-up: `~/.claude/projects/-home-mharman-RustJunosMCP/memory/pfe_batch_subproject.md` §"Known follow-up"
