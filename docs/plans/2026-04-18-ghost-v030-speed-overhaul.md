# Ghost v0.3.0 Speed Overhaul Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Deliver the v0.3.0 speed overhaul designed in `docs/plans/2026-04-18-ghost-v030-speed-overhaul-design.md` - 14 non-predictive improvements that collapse a multi-step browser flow from ~30 s to 2-4 s wall-clock, without CDP, without prediction, undetectable.

**Architecture:** Two new Rust crates (`ghost-cache`, `ghost-intent`) and three new `ghost-core` modules (`idle`, `uia::sta_pool`, `uia::cached_walker`, `input::postmessage`). `ghost-session` gains 9 async methods wired to 10 new additive MCP tools. `ghost-mcp` unchanged except for tool registration. Existing 24 tools untouched.

**Tech Stack:** Rust 2021, `tokio`, `windows` crate 0.58 (UIA + DXGI + Win32 input), `rusqlite` (bundled), `sonic-rs`, `crossbeam-channel`, `blake3`, `serde_json` (legacy path), `tracing`, `criterion` (bench).

**Reference:** Design doc at `docs/plans/2026-04-18-ghost-v030-speed-overhaul-design.md` is the source of truth for API shape, invariants, and budgets. When a task references "design §N", consult that section.

**Principles:**
- **TDD for correctness-critical paths** (cache, locator store, intent FSM, JSONLogic). Glue code gets lighter tests.
- **DRY, YAGNI, frequent commits.** Each task is one commit.
- **Tree buildable after every commit.** `cargo build --workspace` must pass. `cargo test --workspace` must pass (new modules gated behind feature flag if still half-wired is the only exception, and then only transiently).
- **No predictive logic anywhere.** If you feel tempted to "guess ahead," stop and re-read design §2 out-of-scope.

---

## Task 1: Scaffold workspace crates and shared deps

**Files:**
- Modify: `Cargo.toml` (workspace members + workspace.dependencies)
- Create: `crates/ghost-cache/Cargo.toml`
- Create: `crates/ghost-cache/src/lib.rs`
- Create: `crates/ghost-intent/Cargo.toml`
- Create: `crates/ghost-intent/src/lib.rs`

**Step 1: Add dependencies and members**

Add to workspace `Cargo.toml` under `[workspace]`:

```toml
members = [
    "crates/ghost-core",
    "crates/ghost-session",
    "crates/ghost-mcp",
    "crates/ghost-cache",
    "crates/ghost-intent",
]
```

Add under `[workspace.dependencies]`:

```toml
rusqlite = { version = "0.32", features = ["bundled"] }
sonic-rs = "0.3"
crossbeam-channel = "0.5"
blake3 = "1.5"
criterion = { version = "0.5", features = ["html_reports"] }
```

**Step 2: Create `crates/ghost-cache/Cargo.toml`**

```toml
[package]
name = "ghost-cache"
version = "0.3.0"
edition = "2021"

[dependencies]
ghost-core = { path = "../ghost-core" }
tokio.workspace = true
thiserror.workspace = true
tracing.workspace = true
serde.workspace = true
serde_json.workspace = true
rusqlite.workspace = true
blake3.workspace = true
crossbeam-channel.workspace = true
windows.workspace = true

[dev-dependencies]
tempfile = "3"
```

**Step 3: Create `crates/ghost-cache/src/lib.rs`**

```rust
//! ghost-cache: event-driven UIA mirror + SQLite-backed locator store.
pub mod uia_mirror;
pub mod locator_store;
pub mod error;

pub use error::CacheError;
```

Stub modules:
- `uia_mirror.rs`: `pub struct UiaCache;` with empty impl.
- `locator_store.rs`: `pub struct LocatorStore;` with empty impl.
- `error.rs`: `#[derive(thiserror::Error, Debug)] pub enum CacheError { #[error("stub")] Stub }`.

**Step 4: Create `crates/ghost-intent/Cargo.toml`**

```toml
[package]
name = "ghost-intent"
version = "0.3.0"
edition = "2021"

[dependencies]
ghost-core = { path = "../ghost-core" }
ghost-cache = { path = "../ghost-cache" }
tokio.workspace = true
thiserror.workspace = true
tracing.workspace = true
serde.workspace = true
serde_json.workspace = true
```

**Step 5: Create `crates/ghost-intent/src/lib.rs`**

```rust
//! ghost-intent: JSON intent compiler + JSONLogic + FSM executor.
pub mod compiler;
pub mod executor;
pub mod jsonlogic;
pub mod error;

pub use error::IntentError;
```

Stubs for each module with empty structs and `#[error("stub")] Stub` variant in error.

**Step 6: Verify build**

Run: `cargo build --workspace`
Expected: two new crates compile clean with warnings about unused imports.

**Step 7: Commit**

```bash
git add Cargo.toml crates/ghost-cache crates/ghost-intent
git commit -m "feat(v0.3.0): scaffold ghost-cache and ghost-intent crates"
```

---

## Task 2: StaPool - base worker with work queue

**Files:**
- Create: `crates/ghost-core/src/uia/sta_pool.rs`
- Modify: `crates/ghost-core/src/uia/mod.rs` (add `pub mod sta_pool;`)
- Test: `crates/ghost-core/src/uia/sta_pool.rs` (inline `#[cfg(test)] mod tests`)

**Goal:** 4 pre-warmed STA worker threads, each owns its own `IUIAutomation`, job submission returns a future.

**Step 1: Write failing test for happy-path dispatch**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pool_runs_closure_on_worker() {
        let pool = StaPool::new(2).unwrap();
        let result = pool.submit(|_uia| Ok(42i32)).await.unwrap();
        assert_eq!(result, 42);
    }
}
```

Run: `cargo test -p ghost-core sta_pool::tests::pool_runs_closure -- --nocapture`
Expected: FAIL - `StaPool` not defined.

**Step 2: Implement minimal StaPool**

```rust
use crossbeam_channel::{unbounded, Sender};
use std::sync::Arc;
use std::thread;
use tokio::sync::oneshot;
use windows::Win32::UI::Accessibility::{CUIAutomation8, IUIAutomation};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use crate::error::CoreError;

type Job = Box<dyn FnOnce(&IUIAutomation) -> Result<serde_json::Value, CoreError> + Send>;
type JobEnvelope = (Job, oneshot::Sender<Result<serde_json::Value, CoreError>>);

pub struct StaPool {
    tx: Sender<JobEnvelope>,
}

impl StaPool {
    pub fn new(workers: usize) -> Result<Self, CoreError> {
        let (tx, rx) = unbounded::<JobEnvelope>();
        for i in 0..workers {
            let rx = rx.clone();
            thread::Builder::new().name(format!("ghost-sta-{i}")).spawn(move || {
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok(); }
                let uia: IUIAutomation = unsafe {
                    CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                }.expect("CUIAutomation8");
                while let Ok((job, reply)) = rx.recv() {
                    let res = job(&uia);
                    let _ = reply.send(res);
                }
                unsafe { CoUninitialize(); }
            }).map_err(|e| CoreError::ComInit(e.to_string()))?;
        }
        Ok(Self { tx })
    }

    pub async fn submit<F, T>(&self, f: F) -> Result<T, CoreError>
    where
        F: FnOnce(&IUIAutomation) -> Result<T, CoreError> + Send + 'static,
        T: serde::de::DeserializeOwned + serde::Serialize + Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        let job: Job = Box::new(move |uia| {
            let v = f(uia)?;
            Ok(serde_json::to_value(v).unwrap())
        });
        self.tx.send((job, reply_tx)).map_err(|_| CoreError::ComInit("pool dead".into()))?;
        let raw = reply_rx.await.map_err(|_| CoreError::ComInit("worker cancel".into()))??;
        Ok(serde_json::from_value(raw).unwrap())
    }
}
```

**Step 3: Run test**

Run: `cargo test -p ghost-core sta_pool::tests::pool_runs_closure -- --nocapture`
Expected: PASS.

**Step 4: Commit**

```bash
git add crates/ghost-core/src/uia/sta_pool.rs crates/ghost-core/src/uia/mod.rs
git commit -m "feat(core): add StaPool for parallel UIA queries"
```

---

## Task 3: StaPool - panic recovery, circuit breaker, per-job timeout

**Files:**
- Modify: `crates/ghost-core/src/uia/sta_pool.rs`

**Step 1: Write failing test for panic recovery**

```rust
#[tokio::test]
async fn pool_recovers_from_worker_panic() {
    let pool = StaPool::new(1).unwrap();
    let err = pool.submit::<_, i32>(|_| panic!("boom")).await;
    assert!(err.is_err());
    let ok = pool.submit(|_| Ok(7i32)).await.unwrap();
    assert_eq!(ok, 7);
}

#[tokio::test]
async fn pool_enforces_per_job_timeout() {
    let pool = StaPool::new(1).unwrap();
    let err = pool.submit::<_, ()>(|_| { std::thread::sleep(std::time::Duration::from_secs(31)); Ok(()) }).await;
    assert!(err.is_err());
}

#[tokio::test]
async fn pool_circuit_breaker_trips_after_three_panics_in_60s() {
    let pool = StaPool::new(1).unwrap();
    for _ in 0..3 { let _ = pool.submit::<_, ()>(|_| panic!("b")).await; }
    // After trip, submit returns CircuitOpen even for a good job.
    let err = pool.submit(|_| Ok(1i32)).await;
    assert!(matches!(err, Err(CoreError::CircuitOpen)));
}
```

Run: `cargo test -p ghost-core sta_pool::tests -- --nocapture`
Expected: 3 FAIL.

**Step 2: Implement**

- Wrap job execution in `std::panic::catch_unwind`; on panic, send `Err(CoreError::WorkerPanic)` to reply channel, then spawn replacement worker.
- Track panic timestamps in `Arc<Mutex<VecDeque<Instant>>>`. On 3 within 60 s, set `AtomicBool` circuit flag. `submit` short-circuits with `CoreError::CircuitOpen` while flag set. Reset flag after 60 s with no panics.
- Wrap submit in `tokio::time::timeout(Duration::from_secs(30), ...)`; on timeout return `CoreError::JobTimeout`. Note: this doesn't kill the OS thread (impossible cleanly) - it orphans it. Add `tracing::warn!` when orphaning.
- Add variants to `CoreError`: `WorkerPanic(String)`, `JobTimeout`, `CircuitOpen`.

**Step 3: Run tests**

Run: `cargo test -p ghost-core sta_pool::tests -- --nocapture`
Expected: 4 PASS (the original + 3 new).

**Step 4: Commit**

```bash
git add crates/ghost-core/src/uia/sta_pool.rs crates/ghost-core/src/error.rs
git commit -m "feat(core): StaPool panic recovery, circuit breaker, per-job timeout"
```

---

## Task 4: CachedTreeWalker - CacheRequest wrapper + batched property fetch

**Files:**
- Create: `crates/ghost-core/src/uia/cached_walker.rs`
- Modify: `crates/ghost-core/src/uia/mod.rs`

**Goal:** One `IUIAutomationCacheRequest` with all properties we read (`Name`, `ControlType`, `BoundingRectangle`, `RuntimeId`, `IsEnabled`, `IsKeyboardFocusable`, `LocalizedControlType`, `HelpText`, `AutomationId`, `ClassName`). `CachedTreeWalker::walk(root) -> Vec<ElementDescriptor>` returns hydrated descriptors via `Cached*` accessors only. No property-get IPCs during iteration.

**Step 1: Write failing test**

Use Notepad as fixture (already used by existing `examples/diagnose.rs`).

```rust
#[tokio::test]
async fn cached_walker_produces_identical_output_to_manual_walker_but_fewer_ipcs() {
    let _np = spawn_notepad();
    std::thread::sleep(Duration::from_millis(500));
    let pool = StaPool::new(1).unwrap();
    let cached = pool.submit(|uia| CachedTreeWalker::new(uia).walk_all_windows()).await.unwrap();
    let manual = pool.submit(|uia| legacy_walk_all_windows(uia)).await.unwrap();
    assert_eq!(cached.len(), manual.len());
    for (a, b) in cached.iter().zip(manual.iter()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.role, b.role);
        assert_eq!(a.rect, b.rect);
    }
}
```

Run: `cargo test -p ghost-core cached_walker -- --nocapture --ignored` (gate under `#[ignore]` if Notepad unavailable in CI).

**Step 2: Implement `CachedTreeWalker::new(uia)` with `CreateCacheRequest` + `AddProperty` for all 10 properties, `TreeScope::Subtree`, `AutomationElementMode::Full`**

**Step 3: Implement `walk(root)` using `FindAllBuildCache` + condition `TrueCondition`, iterate with `Cached*` accessors, materialize `ElementDescriptor`.**

**Step 4: Run test, verify parity.**

**Step 5: Commit**

```bash
git commit -am "feat(core): CachedTreeWalker with batched UIA CacheRequest"
```

---

## Task 5: CachedTreeWalker - compound conditions for find_by_name / find_by_role

**Files:**
- Modify: `crates/ghost-core/src/uia/cached_walker.rs`

**Goal:** Replace the recursive `search_subtree_by_name` in `tree.rs` with server-side search via `FindAllBuildCache` + `IUIAutomationCondition`.

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn find_by_name_matches_legacy() {
    let _np = spawn_notepad();
    std::thread::sleep(Duration::from_millis(500));
    let pool = StaPool::new(1).unwrap();
    let cached = pool.submit(|uia| CachedTreeWalker::new(uia).find_by_name("File")).await.unwrap();
    assert!(cached.is_some());
    let c = cached.unwrap();
    assert!(c.name.to_lowercase().contains("file"));
}
```

**Step 2: Implement**

Build a `CreatePropertyCondition(UIA_NamePropertyId, VARIANT::bstr(name))` + case-insensitive via custom `CreateAndCondition` with `ControlType != Window`. Use `FindAllBuildCache`, return first match.

For `find_by_role`, map role name (`"edit"`, `"button"`, ...) to `UIA_ControlTypeId` and build `CreatePropertyCondition(UIA_ControlTypePropertyId, ...)`.

**Step 3: Run test.**

Expected: PASS.

**Step 4: Commit**

```bash
git commit -am "feat(core): CachedTreeWalker server-side find_by_name / find_by_role"
```

---

## Task 6: UiaCache - snapshot types, seq, data model

**Files:**
- Modify: `crates/ghost-cache/src/uia_mirror.rs`

**Goal:** Pure-data types, no UIA yet. `Snapshot`, `SnapshotDelta`, `ElementNode`, `Seq`.

**Step 1: Write failing test**

```rust
#[test]
fn diff_empty_snapshots_is_empty_delta() {
    let a = Snapshot::default();
    let b = Snapshot::default();
    let d = a.diff(&b);
    assert!(d.added.is_empty() && d.removed.is_empty() && d.updated.is_empty());
}

#[test]
fn diff_detects_added_and_removed() {
    let a = Snapshot { seq: 1, nodes: vec![node("1", "A"), node("2", "B")] };
    let b = Snapshot { seq: 2, nodes: vec![node("2", "B"), node("3", "C")] };
    let d = a.diff(&b);
    assert_eq!(d.added.len(), 1);
    assert_eq!(d.removed.len(), 1);
    assert_eq!(d.added[0].runtime_id, "3");
    assert_eq!(d.removed[0].runtime_id, "1");
}

#[test]
fn diff_detects_updates_by_runtime_id_when_props_change() {
    let a = Snapshot { seq: 1, nodes: vec![node_full("1", "A", (0, 0, 10, 10))] };
    let b = Snapshot { seq: 2, nodes: vec![node_full("1", "A-renamed", (0, 0, 10, 10))] };
    let d = a.diff(&b);
    assert!(d.added.is_empty() && d.removed.is_empty());
    assert_eq!(d.updated.len(), 1);
}
```

**Step 2: Implement `ElementNode { runtime_id, name, role, rect, ax_checksum, parent_runtime_id }`, `Snapshot { seq, nodes }`, `SnapshotDelta { added, removed, updated, seq }`, `diff(&self, other)` based on `runtime_id` keying.**

`ax_checksum = blake3(runtime_id || name || role || rect_bytes).as_bytes()[..16]`.

**Step 3: Run tests.**

Expected: PASS.

**Step 4: Commit**

```bash
git commit -am "feat(cache): snapshot/delta data types for UIA mirror"
```

---

## Task 7: UiaCache - event subscription + apply_mutation

**Files:**
- Modify: `crates/ghost-cache/src/uia_mirror.rs`

**Goal:** Subscribe to `StructureChangedEventHandler` and `PropertyChangedEventHandler`, maintain `Arc<Mutex<Snapshot>>` mirror, bump seq on each mutation.

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn cache_applies_mutation_and_bumps_seq() {
    let pool = Arc::new(StaPool::new(1).unwrap());
    let cache = UiaCache::start(pool.clone()).await.unwrap();
    let before = cache.seq();
    cache.apply_mutation_for_test(ElementNode::dummy()).await;
    let after = cache.seq();
    assert!(after > before);
}
```

**Step 2: Implement `UiaCache::start(pool)` that submits a job to pool to register event handlers. The handler callbacks use `Arc<Mutex<Snapshot>>` weak-ref to mutate. Add `apply_mutation_for_test` behind `#[cfg(any(test, feature="test-hooks"))]`.**

Key gotcha: UIA event callbacks come in on UIA's threads, so dispatch them to a `crossbeam_channel` and have a dedicated "applier" thread pull from it. This keeps mutation locking simple.

**Step 3: Run test.**

Expected: PASS.

**Step 4: Commit**

```bash
git commit -am "feat(cache): UiaCache event subscription and apply_mutation"
```

---

## Task 8: UiaCache - snapshot + diff public API

**Files:**
- Modify: `crates/ghost-cache/src/uia_mirror.rs`

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn snapshot_returns_noop_delta_when_seq_matches() {
    let pool = Arc::new(StaPool::new(1).unwrap());
    let cache = UiaCache::start(pool).await.unwrap();
    let s1 = cache.snapshot(None, None).await.unwrap();
    let delta = cache.snapshot_delta(None, Some(s1.seq)).await.unwrap();
    assert!(delta.added.is_empty() && delta.removed.is_empty() && delta.updated.is_empty());
    assert_eq!(delta.seq, s1.seq);
}
```

**Step 2: Implement**

- `snapshot(window: Option<&str>, since_seq: Option<u64>) -> Snapshot`: scope to window, lock mirror, clone.
- `snapshot_delta(window, since_seq) -> SnapshotDelta`: if cached `since_seq` snapshot exists, diff. Otherwise fall through to `CachedTreeWalker` + subtract `since_seq` placeholder.
- Retain last N=8 snapshots keyed by seq in `VecDeque` for `since_seq` lookup.

**Step 3: Test passes.**

**Step 4: Commit**

```bash
git commit -am "feat(cache): UiaCache snapshot + delta public API"
```

---

## Task 9: LocatorStore - SQLite schema + migration

**Files:**
- Modify: `crates/ghost-cache/src/locator_store.rs`

**Goal:** Open or create SQLite DB at `{dirs::data_dir()}/ghost/locators.db`, WAL mode, mmap, prepared statements. Schema v1.

**Step 1: Write failing test (uses `tempfile`)**

```rust
#[test]
fn store_opens_creates_schema_and_reports_v1() {
    let tmp = tempfile::tempdir().unwrap();
    let store = LocatorStore::open(tmp.path()).unwrap();
    assert_eq!(store.schema_version(), 1);
}
```

**Step 2: Implement**

```sql
CREATE TABLE IF NOT EXISTS locators (
    id INTEGER PRIMARY KEY,
    app_id TEXT NOT NULL,
    window_class TEXT NOT NULL,
    title_pattern TEXT NOT NULL,
    role TEXT NOT NULL,
    name TEXT NOT NULL,
    rect_left INTEGER NOT NULL,
    rect_top INTEGER NOT NULL,
    rect_right INTEGER NOT NULL,
    rect_bottom INTEGER NOT NULL,
    ax_checksum BLOB NOT NULL,
    last_verified_ms INTEGER NOT NULL,
    hit_count INTEGER NOT NULL DEFAULT 0,
    UNIQUE(app_id, window_class, title_pattern, role, name)
);
CREATE INDEX IF NOT EXISTS idx_locators_lookup ON locators(app_id, window_class);
PRAGMA journal_mode = WAL;
PRAGMA mmap_size = 268435456;  -- 256 MiB
PRAGMA synchronous = NORMAL;
```

Migration: read `PRAGMA user_version`, if 0 apply schema and set to 1.

**Step 3: Test passes.**

**Step 4: Commit**

```bash
git commit -am "feat(cache): LocatorStore SQLite schema v1"
```

---

## Task 10: LocatorStore - upsert/lookup/evict + checksum verify

**Files:**
- Modify: `crates/ghost-cache/src/locator_store.rs`

**Step 1: Write failing tests**

```rust
#[test]
fn upsert_then_lookup_returns_hit() { /* ... */ }
#[test]
fn lookup_with_stale_checksum_returns_miss_and_evicts() { /* ... */ }
#[test]
fn hit_count_increments_on_verified_hit() { /* ... */ }
#[test]
fn store_survives_restart() { /* open, upsert, drop, re-open, lookup */ }
```

**Step 2: Implement `upsert`, `lookup`, `evict(id)`, `verify_or_evict(row, live_checksum)`**

Lookup flow per design §5.4:
- Query by (app_id, window_class, title_pattern_like, role, name).
- If row found, caller passes the live `ax_checksum` from `UiaCache`. If mismatch, evict and return `None`.
- Else increment `hit_count`, update `last_verified_ms`, return rect.

**Step 3: Tests pass.**

**Step 4: Commit**

```bash
git commit -am "feat(cache): LocatorStore upsert/lookup/evict with checksum verify"
```

---

## Task 11: IdleDetector - DXGI duplication held open + stable-frame hash

**Files:**
- Create: `crates/ghost-core/src/capture/idle.rs`
- Modify: `crates/ghost-core/src/capture/mod.rs`

**Step 1: Write failing test**

Hard to unit-test DXGI. Two tests:

```rust
#[tokio::test]
#[ignore]  // requires display
async fn idle_detector_returns_stable_on_static_desktop() {
    let d = IdleDetector::new().unwrap();
    let r = d.wait_stable(3, 1000).await;
    assert!(r.is_ok());
}

#[test]
fn hash_of_identical_frames_matches() {
    let a = vec![0u8; 4 * 4 * 4];
    let b = vec![0u8; 4 * 4 * 4];
    assert_eq!(idle::hash_frame(&a), idle::hash_frame(&b));
}
```

**Step 2: Implement**

- `IdleDetector::new()` initializes `IDXGIFactory1` + enumerates outputs + `DuplicateOutput`, stores `IDXGIOutputDuplication` in struct.
- `wait_stable(stable_frames, timeout_ms)` loops `AcquireNextFrame(16ms)`, maps staging texture, downsamples to 4x4, `blake3::hash` the 64 bytes. Compare to `last_hash`. Increment `stable_count` on match, reset on change. Return `Ok` when `stable_count >= stable_frames`. Return `Err(Timeout)` on overall timeout.
- On `DXGI_ERROR_ACCESS_LOST`, re-init duplication once; if second failure, return `Err(IdleUnavailable)`.

**Step 3: Run the hash-only test.**

Expected: PASS.

**Step 4: Commit**

```bash
git commit -am "feat(core): IdleDetector DXGI-based page-idle signal"
```

---

## Task 12: BackgroundClicker - PostMessage + IsWindow gate

**Files:**
- Create: `crates/ghost-core/src/input/postmessage.rs`
- Modify: `crates/ghost-core/src/input/mod.rs`

**Step 1: Write failing test**

```rust
#[test]
fn click_returns_error_when_hwnd_is_zero() {
    let err = BackgroundClicker::click(HWND(0), (10, 10));
    assert!(err.is_err());
}
```

**Step 2: Implement**

```rust
pub struct BackgroundClicker;

impl BackgroundClicker {
    pub fn click(hwnd: HWND, client_xy: (i32, i32)) -> Result<(), CoreError> {
        unsafe {
            if !IsWindow(hwnd).as_bool() {
                return Err(CoreError::WindowGone);
            }
            let lparam = LPARAM(((client_xy.1 << 16) | (client_xy.0 & 0xFFFF)) as isize);
            PostMessageW(hwnd, WM_LBUTTONDOWN, WPARAM(0x0001), lparam)
                .map_err(|e| CoreError::Input(e.to_string()))?;
            PostMessageW(hwnd, WM_LBUTTONUP, WPARAM(0x0000), lparam)
                .map_err(|e| CoreError::Input(e.to_string()))?;
            Ok(())
        }
    }
}
```

Add `CoreError::WindowGone` variant.

**Step 3: Test passes.**

**Step 4: Commit**

```bash
git commit -am "feat(core): BackgroundClicker via PostMessage"
```

---

## Task 13: IntentCompiler - parse and validate JSON step list

**Files:**
- Modify: `crates/ghost-intent/src/compiler.rs`

**Step 1: Write failing tests**

```rust
#[test]
fn parses_simple_step_list() {
    let json = r#"{"steps":[{"op":"click","target":"Submit"},{"op":"wait_for_text","text":"OK"}]}"#;
    let c = IntentCompiler::compile(json).unwrap();
    assert_eq!(c.ops.len(), 2);
}

#[test]
fn rejects_unknown_op() {
    let json = r#"{"steps":[{"op":"transcendent_meditation"}]}"#;
    assert!(IntentCompiler::compile(json).is_err());
}

#[test]
fn rejects_malformed_abort_if_at_compile_time() {
    let json = r#"{"steps":[],"abort_if":{"&&":[{"==":[1]},{}]}}"#;
    assert!(IntentCompiler::compile(json).is_err());
}
```

**Step 2: Implement**

- `Op` enum with variants: `Click{target}`, `Type{target, text}`, `Press{key}`, `Hotkey{modifiers, key}`, `WaitForText{text, appears, timeout_ms}`, `WaitUntil{condition, timeout_ms}`, `WaitForIdle{stable_frames, timeout_ms}`, `Navigate{url}`, `FocusWindow{name}`, `Screenshot`.
- `CompiledIntent { ops: Vec<Op>, abort_if: Option<JsonLogicExpr>, retry_if: Option<JsonLogicExpr>, max_duration_ms: u64 }`.
- `IntentCompiler::compile(&str)` uses `serde_json` to parse, validates each op by deserializing to the typed `Op`, validates JSONLogic via `jsonlogic::validate` (Task 14).

**Step 3: Tests pass.**

**Step 4: Commit**

```bash
git commit -am "feat(intent): IntentCompiler JSON step parsing + validation"
```

---

## Task 14: JSONLogic evaluator (subset)

**Files:**
- Modify: `crates/ghost-intent/src/jsonlogic.rs`

**Goal:** Implement the subset needed for `abort_if` / `retry_if`: `==`, `!=`, `<`, `<=`, `>`, `>=`, `&&`, `||`, `!`, `in`, `var`, string `contains`. Numbers, strings, booleans, JSON pointers into `state: serde_json::Value` context.

**Step 1: Write failing tests**

```rust
#[test]
fn equals_numbers() { assert!(eval(json!({"==":[1,1]}), &Value::Null).unwrap().as_bool().unwrap()); }
#[test]
fn var_reads_state() {
    let state = json!({"last_error":"timeout"});
    assert!(eval(json!({"==":[{"var":"last_error"},"timeout"]}), &state).unwrap().as_bool().unwrap());
}
#[test]
fn and_short_circuits() { /* ... */ }
#[test]
fn contains_substring() { /* ... */ }
#[test]
fn validate_rejects_wrong_arity() { /* ... */ }
```

**Step 2: Implement `eval(expr: &Value, state: &Value) -> Result<Value, IntentError>` and `validate(expr: &Value) -> Result<(), IntentError>`.**

Keep it small - a `match` on the single operator key at the root of each object. Recursive. ~100 LOC.

**Step 3: Tests pass.**

**Step 4: Commit**

```bash
git commit -am "feat(intent): JSONLogic subset evaluator"
```

---

## Task 15: FsmExecutor - sequential run with abort/retry

**Files:**
- Modify: `crates/ghost-intent/src/executor.rs`

**Goal:** Drive a `CompiledIntent` against an abstract `OpsDispatcher` trait (so tests can mock). Produce a single `IntentResult`.

**Step 1: Write failing tests with a mock dispatcher**

```rust
#[tokio::test]
async fn runs_all_ops_in_order_happy_path() { /* ... */ }

#[tokio::test]
async fn aborts_when_abort_if_becomes_true() { /* ... */ }

#[tokio::test]
async fn retries_on_retry_if_up_to_cap() { /* ... */ }

#[tokio::test]
async fn enforces_max_duration_ms() { /* ... */ }

#[tokio::test]
async fn returns_partial_state_on_op_failure() { /* ... */ }
```

**Step 2: Implement**

```rust
#[async_trait::async_trait]
pub trait OpsDispatcher: Send + Sync {
    async fn dispatch(&self, op: &Op, state: &mut IntentState) -> Result<OpOutcome, IntentError>;
}

pub struct IntentState { pub last_error: Option<String>, pub last_op_index: usize, pub /* other fields */ }
pub enum IntentStatus { Success, Aborted{reason: String}, Failed{at: usize}, Timeout }
pub struct IntentResult { pub status: IntentStatus, pub executed: usize, pub ops_results: Vec<OpOutcome>, pub duration_ms: u64 }

pub struct FsmExecutor<'a> { dispatcher: &'a dyn OpsDispatcher }

impl<'a> FsmExecutor<'a> {
    pub async fn run(&self, intent: &CompiledIntent) -> IntentResult { /* loop, check abort/retry, tokio::time::timeout(max_duration_ms) wrap */ }
}
```

`retry_if` cap = 3 retries per op, exponential backoff 50ms/150ms/450ms.

**Step 3: Tests pass.**

**Step 4: Commit**

```bash
git commit -am "feat(intent): FsmExecutor with abort/retry/timeout"
```

---

## Task 16: Session method - describe_screen_delta

**Files:**
- Modify: `crates/ghost-session/src/session.rs`

**Step 1: Write failing integration test**

```rust
#[tokio::test]
#[ignore]  // requires Notepad
async fn describe_delta_reports_zero_changes_on_idle() {
    let s = Session::new().await.unwrap();
    let a = s.describe_screen_delta(None, None).await.unwrap();
    let b = s.describe_screen_delta(None, Some(a.seq)).await.unwrap();
    assert!(b.added.is_empty() && b.removed.is_empty() && b.updated.is_empty());
}
```

**Step 2: Implement**

Session holds `Arc<UiaCache>` built in `Session::new`. Method delegates to `cache.snapshot_delta(window.as_deref(), since_seq)`.

**Step 3: Test passes.**

**Step 4: Commit**

```bash
git commit -am "feat(session): describe_screen_delta via UiaCache"
```

---

## Task 17: Session methods - wait_until + wait_for_idle

**Files:**
- Modify: `crates/ghost-session/src/session.rs`

**Step 1: Failing tests**

```rust
#[tokio::test]
async fn wait_until_resolves_when_condition_true() { /* poll a test state with a JSONLogic condition that flips true */ }
#[tokio::test]
async fn wait_until_times_out() { /* condition never true */ }
#[tokio::test]
#[ignore]
async fn wait_for_idle_returns_within_500ms_on_static_desktop() { /* ... */ }
```

**Step 2: Implement**

- `wait_until(condition, timeout_ms, poll_ms)`: build a state from current cache snapshot + `last_error`; eval condition every `poll_ms`; return on true or timeout. Default `poll_ms = 50`.
- `wait_for_idle(window, stable_frames, timeout_ms)`: delegate to `IdleDetector` held on Session.

**Step 3: Tests pass.**

**Step 4: Commit**

```bash
git commit -am "feat(session): wait_until and wait_for_idle"
```

---

## Task 18: Session methods - navigate_and_wait, click_and_wait_for_text, fill_form

**Files:**
- Modify: `crates/ghost-session/src/session.rs`

**Step 1: Failing integration tests against a local `file://` HTML fixture**

Create `tests/fixtures/form.html` with a known form and async-loaded text.

```rust
#[tokio::test]
#[ignore]
async fn navigate_and_wait_loads_page_and_finds_body_text() { /* ... */ }
#[tokio::test]
#[ignore]
async fn click_and_wait_for_text_resolves_on_appear() { /* ... */ }
#[tokio::test]
#[ignore]
async fn fill_form_fills_all_fields_and_submits() { /* ... */ }
```

**Step 2: Implement**

- `navigate_and_wait`: focus browser window, set address bar via existing `type` primitive, `Enter`, then `wait_for_idle` OR `wait_until_text(wait_for.text)`.
- `click_and_wait_for_text`: resolve target via `LocatorStore`→`CachedTreeWalker`, click, then `wait_until(condition: contains(screen_text, wait_for_text) == appears)`.
- `fill_form`: iterate fields, for each: click target, `SendInput` text. Optional submit: click submit target + wait_for_idle.

**Step 3: Tests pass against the fixture on Edge.**

**Step 4: Commit**

```bash
git commit -am "feat(session): macro primitives navigate_and_wait, click_and_wait_for_text, fill_form"
```

---

## Task 19: Session method - execute_intent

**Files:**
- Modify: `crates/ghost-session/src/session.rs`

**Step 1: Failing test**

```rust
#[tokio::test]
#[ignore]
async fn execute_intent_runs_three_op_notepad_sequence() { /* click File, click New, wait for blank */ }
#[tokio::test]
async fn execute_intent_aborts_on_abort_if_match() { /* ... */ }
```

**Step 2: Implement**

Session has a `SessionOpsDispatcher` that implements `OpsDispatcher` by routing each `Op` to existing session methods (click, type, press, wait_for_text, etc).

```rust
pub async fn execute_intent(&self, json: &str) -> Result<IntentResult, SessionError> {
    let intent = IntentCompiler::compile(json)?;
    let dispatcher = SessionOpsDispatcher::new(self);
    let executor = FsmExecutor::new(&dispatcher);
    Ok(executor.run(&intent).await)
}
```

**Step 3: Tests pass.**

**Step 4: Commit**

```bash
git commit -am "feat(session): execute_intent wires FsmExecutor to session primitives"
```

---

## Task 20: Session methods - click_background, cache_stats, cache_invalidate

**Files:**
- Modify: `crates/ghost-session/src/session.rs`

**Step 1: Tests**

```rust
#[test] fn cache_stats_returns_zeros_on_fresh_session() {}
#[tokio::test] async fn cache_invalidate_all_resets_stats() {}
#[tokio::test]
#[ignore]
async fn click_background_does_not_change_foreground() { /* GetForegroundWindow before/after */ }
```

**Step 2: Implement using `UiaCache::stats()`, `LocatorStore::row_count()`, `StaPool` metrics, etc.**

**Step 3: Tests pass.**

**Step 4: Commit**

```bash
git commit -am "feat(session): click_background, cache_stats, cache_invalidate"
```

---

## Task 21: MCP tool registration - 10 new tools

**Files:**
- Modify: `crates/ghost-mcp/src/main.rs` (or wherever tool table lives)
- Modify: `crates/ghost-mcp/src/tools/mod.rs`

**Step 1: Write a test that lists tool names and asserts all 10 new ones are present**

```rust
#[test]
fn all_v030_tools_registered() {
    let names = list_registered_tools();
    for t in ["ghost_wait_until","ghost_wait_for_idle","ghost_navigate_and_wait",
              "ghost_click_and_wait_for_text","ghost_fill_form","ghost_execute_intent",
              "ghost_describe_screen_delta","ghost_click_background",
              "ghost_cache_stats","ghost_cache_invalidate"] {
        assert!(names.contains(&t.to_string()), "missing {t}");
    }
}
```

**Step 2: Register each tool with JSON schema matching design §4. Each handler calls the matching `Session` method and returns its `Result` as JSON.**

Critical: schemas need `additionalProperties: false` and mark `timeout_ms`, `target` etc as required where appropriate.

**Step 3: Test passes. Smoke via `cargo run -p ghost-mcp` + MCP inspector tool list.**

**Step 4: Commit**

```bash
git commit -am "feat(mcp): register v0.3.0 tools"
```

---

## Task 22: sonic-rs migration for large MCP responses

**Files:**
- Modify: `crates/ghost-mcp/src/main.rs` (or response encoder)

**Step 1: Write a bench comparing serde_json vs sonic-rs on a 75 KB payload**

```rust
// benches/serialize.rs using criterion
fn bench_serialize(c: &mut Criterion) {
    let payload = load_fixture_75kb();
    c.bench_function("serde_json", |b| b.iter(|| serde_json::to_vec(&payload).unwrap()));
    c.bench_function("sonic_rs", |b| b.iter(|| sonic_rs::to_vec(&payload).unwrap()));
}
```

**Step 2: Add a helper `encode_response(value) -> Vec<u8>` that picks sonic-rs when serialized size likely > 4 KB, serde_json otherwise (use a cheap heuristic: `if matches!(value, Value::Array(a) if a.len() > 20)`). On sonic-rs encode error, fall back to serde_json and log.**

**Step 3: Run bench, record numbers in `docs/benches/v030-baseline.md`.**

Expected: sonic-rs 3-5x faster on the large payload.

**Step 4: Commit**

```bash
git commit -am "perf(mcp): sonic-rs for responses >4KB with fallback"
```

---

## Task 23: Integration tests - Notepad scenarios

**Files:**
- Create: `crates/ghost-session/tests/notepad_flow.rs`

**Step 1: Write the full Notepad integration suite per design §7 Layer 2**

```rust
#[tokio::test] #[ignore]
async fn delta_describe_tracks_typed_text() { /* ... */ }
#[tokio::test] #[ignore]
async fn execute_intent_find_replace_flow() { /* ... */ }
#[tokio::test] #[ignore]
async fn locator_cache_lifecycle_cold_warm_drift_restart() { /* ... */ }
#[tokio::test] #[ignore]
async fn click_background_does_not_steal_foreground() { /* ... */ }
```

All gated `#[ignore]` so CI-unavailable environments skip; run locally via `cargo test -- --ignored`.

**Step 2: Implement test fixtures; reuse `spawn_notepad` helper from existing `examples/diagnose.rs`.**

**Step 3: All tests pass locally.**

**Step 4: Commit**

```bash
git commit -am "test: Notepad integration suite for v0.3.0"
```

---

## Task 24: Integration tests - Edge / Comet / Chrome

**Files:**
- Create: `crates/ghost-session/tests/browser_flow.rs`
- Create: `crates/ghost-session/tests/fixtures/form.html`

**Step 1: Local HTML fixture with a 2-field form + async text that appears after 200 ms.**

**Step 2: Test matrix: Edge, Comet, Chrome. Each test detects whether the browser is installed and skips otherwise.**

```rust
#[tokio::test] #[ignore]
async fn navigate_and_wait_resolves_on_edge() { run_on("msedge").await; }
#[tokio::test] #[ignore]
async fn execute_intent_form_login_on_comet() { run_on("comet").await; }
#[tokio::test] #[ignore]
async fn describe_delta_small_payload_on_dom_change_across_browsers() { /* ... */ }
```

**Step 3: All pass on at least Edge (required), skip Comet/Chrome if not installed.**

**Step 4: Commit**

```bash
git commit -am "test: browser flow integration on local fixture"
```

---

## Task 25: Criterion benchmarks + CI budgets

**Files:**
- Create: `crates/ghost-core/benches/uia_walks.rs`
- Create: `crates/ghost-session/benches/hot_paths.rs`
- Create: `docs/benches/v030-baseline.md` (committed, frozen)
- Modify: `.github/workflows/ci.yml` (if repo has CI) or add `scripts/bench-check.sh`

**Step 1: Implement benches for the 7 operations in design §7 Layer 4.**

**Step 2: Run `cargo bench --bench hot_paths --save-baseline v030`, record numbers, assert each meets its budget. Commit baseline JSON to `target/criterion/...` path under `docs/benches/` (copy/paste, criterion's default target/ dir isn't committed).**

**Step 3: Add a `scripts/bench-check.sh` that compares current run to baseline, fails with non-zero exit if any benchmark regresses >20%.**

**Step 4: Document the budgets and how to re-baseline in `docs/benches/README.md`.**

**Step 5: Commit**

```bash
git commit -am "perf: criterion benches + v0.3.0 budgets with CI gate"
```

---

## Task 26: Failure injection tests (`ghost-chaos` feature)

**Files:**
- Modify: each relevant crate's `Cargo.toml` to add `chaos = []` feature
- Modify: components to expose chaos hooks behind `#[cfg(feature = "chaos")]`
- Create: `crates/ghost-session/tests/chaos.rs`

**Step 1: Add chaos hooks**

- `UiaCache::chaos_drop_events()` - simulates COM disconnect.
- `LocatorStore::chaos_lock_db()` - wraps a long transaction.
- `StaPool::chaos_kill_worker(i)` - panics worker i's next job.
- `IdleDetector::chaos_surface_lost()` - simulates DXGI_ERROR_ACCESS_LOST.

**Step 2: Write tests per design §7 Layer 5**

```rust
#[tokio::test] #[ignore]
async fn cache_recovers_from_com_disconnect() { /* ... */ }
#[tokio::test] #[ignore]
async fn locator_store_degrades_to_memory_when_locked() { /* ... */ }
#[tokio::test] #[ignore]
async fn sta_pool_circuit_trips_on_repeated_panics() { /* ... */ }
#[tokio::test] #[ignore]
async fn idle_detector_reinits_on_surface_lost() { /* ... */ }
#[tokio::test]
async fn fsm_halts_on_max_duration_when_retry_if_always_true() { /* ... */ }
```

**Step 3: All pass when run with `cargo test --features chaos -- --ignored`.**

**Step 4: Commit**

```bash
git commit -am "test: failure injection suite behind chaos feature"
```

---

## Task 27: Manual kimi.com validation, CHANGELOG, version bump

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `Cargo.toml` (workspace version), each crate's Cargo.toml
- Create: `docs/benches/kimi-flow-v030.md`

**Step 1: Manual run**

Start Ghost MCP against Claude. Submit one `execute_intent` call describing:

```json
{
  "steps": [
    {"op": "focus_window", "name": "Microsoft Edge"},
    {"op": "hotkey", "modifiers": ["Ctrl"], "key": "t"},
    {"op": "wait_for_idle", "timeout_ms": 2000},
    {"op": "type", "target": "Address and search bar", "text": "https://www.kimi.com"},
    {"op": "press", "key": "Enter"},
    {"op": "wait_for_idle", "timeout_ms": 6000},
    {"op": "click", "target": "Ask Anything..."},
    {"op": "type", "target": "Ask Anything...", "text": "What is agentics?"},
    {"op": "press", "key": "Enter"},
    {"op": "wait_for_idle", "timeout_ms": 15000}
  ],
  "abort_if": {"==": [{"var": "last_error"}, "window_gone"]},
  "max_duration_ms": 30000
}
```

Record wall-clock. Target: <4 s Ghost-side (excluding model think time).

**Step 2: Record result in `docs/benches/kimi-flow-v030.md` with before/after comparison vs the ~30 s session recorded today.**

**Step 3: Bump versions to 0.3.0 across workspace.**

**Step 4: Write CHANGELOG.md entry summarizing the 14 improvements and pointing to the design doc.**

**Step 5: Commit and tag**

```bash
git add CHANGELOG.md Cargo.toml crates/*/Cargo.toml docs/benches/kimi-flow-v030.md
git commit -m "release: v0.3.0 speed overhaul"
git tag v0.3.0
```

---

## Ordering / critical path

```
1 (scaffold)
 ├─ 2,3 (StaPool)
 │   ├─ 4,5 (CachedTreeWalker)
 │   │   └─ 6,7,8 (UiaCache)
 │   │       └─ 9,10 (LocatorStore)
 │   │           └─ 13,14,15 (Intent system)
 │   │               └─ 16..20 (Session methods)
 │   │                   └─ 21 (MCP tools)
 │   │                       └─ 22..26 (perf + tests + chaos)
 │   │                           └─ 27 (release)
 │   └─ 11,12 (IdleDetector, BackgroundClicker - parallel branch)
```

Tasks 2-5 and 11-12 can run in parallel if multiple contributors; otherwise sequential per the graph.

## Non-negotiable rules for execution

1. **Never introduce predictive logic.** No "if the last 3 clicks succeeded, pre-fetch the next AX subtree." The FSM runs ops the agent gave it, nothing more.
2. **Never silently degrade without telemetry.** Every fallback path increments a counter exposed via `ghost_cache_stats`.
3. **Never commit with failing `cargo test --workspace`.** Gate unfinished work behind `#[cfg(feature = "wip-xyz")]` if needed, but main-branch tests always pass.
4. **Never mutate the existing 24 tools.** v0.3.0 is purely additive. If you find yourself changing a v0.2 tool signature, stop and reconsider.
5. **Always verify AX checksum before cached click.** Design §6 invariant 1.
