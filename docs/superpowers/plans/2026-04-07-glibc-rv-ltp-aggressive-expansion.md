# glibc-rv LTP Aggressive Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expand glibc-rv LTP score coverage through aggressive pending→curated→score promotion while fixing `shmctl02` with the smallest shared-memory control semantic change needed for stable promotion.

**Architecture:** The implementation is split into two small tracks that meet at the promotion pipeline. First, validate and run the existing glibc-rv pending/curated/score machinery exactly as the repository already expects, collecting candidate and blocker evidence without changing semantics. Second, add a narrow `sys_shmctl` fix plus unit coverage so `shmctl02` can move from a known blocker toward promotion, then rerun the same promotion gates and update the runbook.

**Tech Stack:** Rust (`crates/syscall`), Bash (`tools/dev/run_oscomp_stage2.sh`, `tools/dev/test_run_oscomp_stage2.sh`), LTP whitelist files, QEMU stage2 runner

---

## File Structure

| File | Responsibility |
| --- | --- |
| `tools/oscomp/ltp/pending_whitelist_glibc_rv.txt` | glibc-rv discovery queue for cases still under evaluation |
| `tools/oscomp/ltp/pending_blacklist_glibc_rv.txt` | glibc-rv exclusions while pending promotion runs classify results |
| `tools/oscomp/ltp/curated_whitelist_glibc_rv.txt` | glibc-rv stability layer that must stay `bad=0 conf=0` |
| `tools/oscomp/ltp/curated_blacklist_glibc_rv.txt` | glibc-rv curated exclusions, including known blockers like `shmctl02` before a fix |
| `tools/oscomp/ltp/score_whitelist_glibc_rv.txt` | glibc-rv score-bearing whitelist protected by the score gate |
| `tools/oscomp/ltp/score_blacklist_glibc_rv.txt` | glibc-rv score exclusions preserved unless a gate proves promotion is safe |
| `tools/dev/run_oscomp_stage2.sh` | pending / curated / score orchestration, candidate extraction, and promotion application |
| `tools/dev/test_run_oscomp_stage2.sh` | shell regression guard for stage2 runner wiring and promotion hooks |
| `crates/syscall/src/lib.rs` | `sys_shmctl` implementation, shared-memory state structs, and syscall unit tests |
| `AGENTS.md` | runbook snapshot of score state, blockers, and next validation path |

---

### Task 1: Verify glibc-rv promotion pipeline before semantic changes

**Files:**
- Read: `tools/dev/run_oscomp_stage2.sh:1197-1237`
- Read: `tools/dev/run_oscomp_stage2.sh:1416-1503`
- Read: `tools/dev/run_oscomp_stage2.sh:2198-2225`
- Test: `tools/dev/test_run_oscomp_stage2.sh`
- Modify if promotion output changes during apply: `tools/oscomp/ltp/pending_whitelist_glibc_rv.txt`
- Modify if promotion output changes during apply: `tools/oscomp/ltp/curated_whitelist_glibc_rv.txt`
- Modify if promotion output changes during apply: `tools/oscomp/ltp/curated_blacklist_glibc_rv.txt`
- Modify if promotion output changes during apply: `tools/oscomp/ltp/score_whitelist_glibc_rv.txt`
- Modify if promotion output changes during apply: `tools/oscomp/ltp/score_blacklist_glibc_rv.txt`

- [ ] **Step 1: Verify the shell guard still covers glibc-rv promotion wiring**

```bash
bash tools/dev/test_run_oscomp_stage2.sh
```

Expected: exits `0` with no `FAIL:` lines, confirming the runner still exposes `ltp-riscv-pending`, `ltp-riscv-curated`, `curated->score promoted`, and glibc-rv whitelist file wiring.

- [ ] **Step 2: Run the failing discovery gate for glibc-rv pending**

```bash
TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv-pending
```

Expected: the run completes and prints either `pending->curated promoted` or a review/alarm line showing which pending glibc-rv cases remain bad/conf; before semantic fixes, `shmctl02` is allowed to remain blocked.

- [ ] **Step 3: Run the curated stability gate for glibc-rv**

```bash
TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv-curated
```

Expected: the run reaches `whuse-oscomp-suite-done` and either keeps curated stable or reports a concrete curated regression review file; no score file should be changed outside the built-in apply path.

- [ ] **Step 4: Run the score gate for glibc-rv candidates**

```bash
TIMEOUT_SECS=2400 WHUSE_LTP_PROFILE=score WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv
```

Expected: the run prints `score-gate summary` and either `curated->score promoted N case(s)` or an explicit `score gate failed for candidate batch` message that preserves the current score files.

- [ ] **Step 5: Inspect the resulting list delta before touching semantics**

```bash
git diff -- tools/oscomp/ltp/pending_whitelist_glibc_rv.txt tools/oscomp/ltp/curated_whitelist_glibc_rv.txt tools/oscomp/ltp/curated_blacklist_glibc_rv.txt tools/oscomp/ltp/score_whitelist_glibc_rv.txt tools/oscomp/ltp/score_blacklist_glibc_rv.txt
```

Expected: shows only promotion-driven list changes. If there is no diff, the pipeline produced evidence without promotion and `shmctl02` stays the primary semantic target.

- [ ] **Step 6: Commit the promotion-only checkpoint if list files changed**

```bash
git add tools/oscomp/ltp/pending_whitelist_glibc_rv.txt tools/oscomp/ltp/curated_whitelist_glibc_rv.txt tools/oscomp/ltp/curated_blacklist_glibc_rv.txt tools/oscomp/ltp/score_whitelist_glibc_rv.txt tools/oscomp/ltp/score_blacklist_glibc_rv.txt
git commit -m "ltp-rv: promote glibc candidates before shmctl02 fix"
```

Expected: creates a commit only when the whitelist files changed. If `git diff --cached --quiet` would be empty, skip this step and continue without a commit.

---

### Task 2: Add failing syscall tests for `shmctl02` expectations

**Files:**
- Modify: `crates/syscall/src/lib.rs:11908-20900`
- Test: `crates/syscall/src/lib.rs:20752-20827`

- [ ] **Step 1: Add a failing test for `IPC_SET` updating shared-memory metadata**

Insert the following test inside `mod tests` in `crates/syscall/src/lib.rs` near the other SysV IPC tests:

```rust
    #[test]
    fn shmctl_ipc_set_updates_owner_mode_and_ipc_stat_reflects_it() {
        ensure_test_hal();
        let dispatcher = SyscallDispatcher::new();
        let mut procs = ProcessTable::new();
        let init = procs.spawn_init("init", 0x1000);
        procs.set_current(init).unwrap();
        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init, init);
        scheduler.start();
        let mut vfs = KernelVfs::new();

        let shmid = dispatcher.dispatch(
            SYS_SHMGET,
            SyscallArgs([0x3344, 4096, super::IPC_CREAT | 0o600, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        ) as usize;

        let mut set_buf = [0u8; core::mem::size_of::<super::ShmidDs>()];
        set_buf[0..4].copy_from_slice(&77u32.to_le_bytes());
        set_buf[4..8].copy_from_slice(&0o644u32.to_le_bytes());
        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0xb200, &set_buf);

        let rc = dispatcher.dispatch(
            SYS_SHMCTL,
            SyscallArgs([shmid, super::IPC_SET as usize, 0xb200, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        );
        assert_eq!(rc, 0);

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0xb280, &[0u8; core::mem::size_of::<super::ShmidDs>()]);

        let stat_rc = dispatcher.dispatch(
            SYS_SHMCTL,
            SyscallArgs([shmid, super::IPC_STAT as usize, 0xb280, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        );
        assert_eq!(stat_rc, 0);

        let stat = procs
            .current()
            .unwrap()
            .read_user_bytes(0xb280, core::mem::size_of::<super::ShmidDs>())
            .unwrap();
        let read_u32 = |off: usize| -> u32 {
            u32::from_le_bytes(stat[off..off + 4].try_into().unwrap())
        };
        assert_eq!(read_u32(0), 77);
        assert_eq!(read_u32(4) & 0o777, 0o644);
    }
```

- [ ] **Step 2: Run the new `IPC_SET` test to verify it fails first**

```bash
cargo test -p syscall shmctl_ipc_set_updates_owner_mode_and_ipc_stat_reflects_it -- --exact
```

Expected: FAIL because current `sys_shmctl` does not implement `IPC_SET` semantics or expose the updated metadata in `IPC_STAT`.

- [ ] **Step 3: Add a failing test for `IPC_RMID` + attached segment behavior**

Add this second test next to the first one:

```rust
    #[test]
    fn shmctl_ipc_rmid_keeps_attached_segment_alive_until_detach() {
        ensure_test_hal();
        let dispatcher = SyscallDispatcher::new();
        let mut procs = ProcessTable::new();
        let init = procs.spawn_init("init", 0x1000);
        procs.set_current(init).unwrap();
        let mut scheduler = Scheduler::new();
        scheduler.spawn("init", init, init);
        scheduler.start();
        let mut vfs = KernelVfs::new();

        let shmid = dispatcher.dispatch(
            SYS_SHMGET,
            SyscallArgs([0x4455, 4096, super::IPC_CREAT | 0o600, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        ) as usize;

        let attach_addr = dispatcher.dispatch(
            SYS_SHMAT,
            SyscallArgs([shmid, 0, 0, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        ) as usize;
        assert_ne!(attach_addr, 0);

        let rm_rc = dispatcher.dispatch(
            SYS_SHMCTL,
            SyscallArgs([shmid, super::IPC_RMID as usize, 0, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        );
        assert_eq!(rm_rc, 0);

        procs
            .current_mut()
            .unwrap()
            .address_space
            .install_bytes(0xb300, &[0u8; core::mem::size_of::<super::ShmidDs>()]);

        let stat_after_rmid = dispatcher.dispatch(
            SYS_SHMCTL,
            SyscallArgs([shmid, super::IPC_STAT as usize, 0xb300, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        );
        assert_eq!(stat_after_rmid, 0);

        let dt_rc = dispatcher.dispatch(
            SYS_SHMDT,
            SyscallArgs([attach_addr, 0, 0, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        );
        assert_eq!(dt_rc, 0);

        let stat_after_detach = dispatcher.dispatch(
            SYS_SHMCTL,
            SyscallArgs([shmid, super::IPC_STAT as usize, 0xb300, 0, 0, 0]),
            &mut procs,
            &mut scheduler,
            &mut vfs,
        );
        assert_eq!(stat_after_detach, -(super::ENOENT as isize));
    }
```

- [ ] **Step 4: Run the new `IPC_RMID` test to verify current behavior**

```bash
cargo test -p syscall shmctl_ipc_rmid_keeps_attached_segment_alive_until_detach -- --exact
```

Expected: either FAIL directly or expose the exact post-`IPC_RMID` behavior. Keep the failure text because it defines the minimal fix required next.

- [ ] **Step 5: Commit the failing tests**

```bash
git add crates/syscall/src/lib.rs
git commit -m "test(syscall): capture shmctl02 shared-memory expectations"
```

Expected: commit contains only the new failing tests.

---

### Task 3: Implement the minimal `sys_shmctl` semantic fix

**Files:**
- Modify: `crates/syscall/src/lib.rs:969-1024`
- Modify: `crates/syscall/src/lib.rs:9292-9358`
- Test: `crates/syscall/src/lib.rs:11908-20900`

- [ ] **Step 1: Extend the shared-memory metadata structures to store owner/mode**

Update the shared-memory structs in `crates/syscall/src/lib.rs` to this shape:

```rust
#[repr(C)]
struct ShmidDs {
    shm_perm_uid: u32,
    shm_perm_mode: u32,
    shm_segsz: usize,
    shm_nattch: usize,
    shm_cpid: usize,
    shm_lpid: usize,
    shm_atime: usize,
    shm_dtime: usize,
    shm_ctime: usize,
}

impl Default for ShmidDs {
    fn default() -> Self {
        ShmidDs {
            shm_perm_uid: 0,
            shm_perm_mode: 0,
            shm_segsz: 0,
            shm_nattch: 0,
            shm_cpid: 0,
            shm_lpid: 0,
            shm_atime: 0,
            shm_dtime: 0,
            shm_ctime: 0,
        }
    }
}

struct ShmSegment {
    data: alloc::sync::Arc<Mutex<Vec<u8>>>,
    key: i32,
    creator_pid: usize,
    owner_uid: u32,
    mode: u16,
    attach_count: usize,
    attachments: Vec<ShmAttachment>,
    destroyed: bool,
}

impl ShmSegment {
    fn new(key: i32, size: usize, creator_pid: usize, owner_uid: u32, mode: u16) -> Self {
        ShmSegment {
            data: alloc::sync::Arc::new(Mutex::new(vec![0; size.max(1)])),
            key,
            creator_pid,
            owner_uid,
            mode,
            attach_count: 0,
            attachments: Vec::new(),
            destroyed: false,
        }
    }
}
```

- [ ] **Step 2: Update `sys_shmget` to initialize the new metadata**

Replace the `ShmSegment::new(...)` call in `sys_shmget` with:

```rust
        let caller_uid = procs.current()?.euid;
        let mode = (flags & 0o777) as u16;
        let segment = ShmSegment::new(key, size as usize, pid, caller_uid, mode);
```

Expected: new shared-memory segments preserve creator uid and mode at creation time.

- [ ] **Step 3: Implement `IPC_SET` and enrich `IPC_STAT` in `sys_shmctl`**

Replace the `match cmd` body inside `sys_shmctl` with:

```rust
        match cmd {
            IPC_RMID => {
                let (key, attach_count) = {
                    let segment = state.segments.get(&id).ok_or(ENOENT)?;
                    (segment.key, segment.attach_count)
                };
                let segment = state.segments.get_mut(&id).ok_or(ENOENT)?;
                segment.destroyed = true;
                if key != IPC_PRIVATE {
                    state.keys.remove(&key);
                }
                if attach_count == 0 {
                    state.segments.remove(&id);
                }
            }
            IPC_SET => {
                if buf == 0 {
                    return Err(EFAULT);
                }
                let raw = procs
                    .current()?
                    .read_user_bytes(buf, core::mem::size_of::<ShmidDs>())
                    .map_err(|_| EFAULT)?;
                let owner_uid = u32::from_le_bytes(raw[0..4].try_into().unwrap());
                let mode = u32::from_le_bytes(raw[4..8].try_into().unwrap()) as u16;
                let segment = state.segments.get_mut(&id).ok_or(ENOENT)?;
                segment.owner_uid = owner_uid;
                segment.mode = mode & 0o777;
            }
            IPC_STAT => {
                if buf == 0 {
                    return Err(EFAULT);
                }
                let segment = state.segments.get(&id).ok_or(ENOENT)?;
                let info = ShmidDs {
                    shm_perm_uid: segment.owner_uid,
                    shm_perm_mode: segment.mode as u32,
                    shm_segsz: segment.data.lock().len(),
                    shm_nattch: segment.attach_count,
                    shm_cpid: segment.creator_pid,
                    shm_lpid: 0,
                    shm_atime: 0,
                    shm_dtime: 0,
                    shm_ctime: 0,
                };
                let bytes: &[u8] = unsafe {
                    core::slice::from_raw_parts(
                        &info as *const ShmidDs as *const u8,
                        core::mem::size_of::<ShmidDs>(),
                    )
                };
                procs
                    .current_mut()?
                    .write_user_bytes(buf, bytes)
                    .map_err(|_| EFAULT)?;
            }
            _ => return Err(EINVAL),
        }
```

Expected: `IPC_SET` is no longer a stub, `IPC_STAT` returns the same metadata layout the tests read, and `IPC_RMID` still delays final removal until detach when attachments remain.

- [ ] **Step 4: Run the targeted `shmctl` tests and make them pass**

```bash
cargo test -p syscall shmctl_ipc_set_updates_owner_mode_and_ipc_stat_reflects_it -- --exact && cargo test -p syscall shmctl_ipc_rmid_keeps_attached_segment_alive_until_detach -- --exact
```

Expected: both tests PASS.

- [ ] **Step 5: Run the nearby existing SysV IPC tests to avoid regressions**

```bash
cargo test -p syscall msgrcv_empty_queue_blocks_with_eagain_and_marks_scheduler_blocked -- --exact && cargo test -p syscall semop_unavailable_resource_blocks_with_eagain_and_marks_scheduler_blocked -- --exact
```

Expected: both tests PASS, proving the `shmctl` edit did not break recent message queue or semaphore behavior.

- [ ] **Step 6: Commit the minimal semantic fix**

```bash
git add crates/syscall/src/lib.rs
git commit -m "syscall: implement shmctl metadata updates for shmctl02"
```

Expected: commit includes the shared-memory struct updates, `sys_shmctl` implementation, and the new passing tests.

---

### Task 4: Re-run glibc-rv promotion after the `shmctl02` fix

**Files:**
- Test: `tools/dev/test_run_oscomp_stage2.sh`
- Modify if promotion output changes during apply: `tools/oscomp/ltp/pending_whitelist_glibc_rv.txt`
- Modify if promotion output changes during apply: `tools/oscomp/ltp/curated_whitelist_glibc_rv.txt`
- Modify if promotion output changes during apply: `tools/oscomp/ltp/curated_blacklist_glibc_rv.txt`
- Modify if promotion output changes during apply: `tools/oscomp/ltp/score_whitelist_glibc_rv.txt`
- Modify if promotion output changes during apply: `tools/oscomp/ltp/score_blacklist_glibc_rv.txt`

- [ ] **Step 1: Re-run the shell guard after the syscall change**

```bash
bash tools/dev/test_run_oscomp_stage2.sh
```

Expected: exits `0` with no `FAIL:` lines.

- [ ] **Step 2: Re-run glibc-rv pending with the same aggressive gate**

```bash
TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv-pending
```

Expected: if `shmctl02` is now healthy, the runner can include it in the `pending->curated promoted` set; otherwise it should fail in a narrower and more actionable way than before.

- [ ] **Step 3: Re-run glibc-rv curated**

```bash
TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv-curated
```

Expected: curated stays at `bad=0 conf=0` or emits an explicit review file naming any residual blockers.

- [ ] **Step 4: Re-run the glibc-rv score gate**

```bash
TIMEOUT_SECS=2400 WHUSE_LTP_PROFILE=score WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv
```

Expected: either promotes a bounded candidate batch into `tools/oscomp/ltp/score_whitelist_glibc_rv.txt` or preserves score files with a clear `score alarm`/candidate review outcome.

- [ ] **Step 5: Inspect the post-fix whitelist delta**

```bash
git diff -- tools/oscomp/ltp/pending_whitelist_glibc_rv.txt tools/oscomp/ltp/curated_whitelist_glibc_rv.txt tools/oscomp/ltp/curated_blacklist_glibc_rv.txt tools/oscomp/ltp/score_whitelist_glibc_rv.txt tools/oscomp/ltp/score_blacklist_glibc_rv.txt
```

Expected: shows exactly which glibc-rv cases moved. Capture whether `shmctl02` left the blocker set.

- [ ] **Step 6: Commit the post-fix promotion wave if list files changed**

```bash
git add tools/oscomp/ltp/pending_whitelist_glibc_rv.txt tools/oscomp/ltp/curated_whitelist_glibc_rv.txt tools/oscomp/ltp/curated_blacklist_glibc_rv.txt tools/oscomp/ltp/score_whitelist_glibc_rv.txt tools/oscomp/ltp/score_blacklist_glibc_rv.txt
git commit -m "ltp-rv: advance glibc promotion after shmctl02 fix"
```

Expected: creates a promotion-wave commit only when the list files changed.

---

### Task 5: Update the runbook with the new glibc-rv state

**Files:**
- Modify: `AGENTS.md:230-253`

- [ ] **Step 1: Update the current local state bullets**

Edit the bullets under `AGENTS.md` section `4.4` so they reflect the actual outcome of Task 4. Use this template, replacing bracketed text with the measured result:

```md
- RV LTP pending/curated/score pipeline is active for dual runtime (`musl` + `glibc`) with conservative score auto-promotion (`batch<=8` by default).
- glibc-rv current wave promoted `[CASE LIST OR COUNT]` after rerunning pending/curated/score post-`shmctl02` fix.
- `shmctl02` is now `[promoted to curated | promoted to score | still blocked with narrowed failure mode: ...]`.
```

Expected: `AGENTS.md` records actual post-run state rather than the old unresolved status line.

- [ ] **Step 2: Update the immediate engineering goal bullets to match the new focus**

Replace the glibc-rv goal bullet in section `5.1` with:

```md
- For `glibc-rv`, keep score-whitelist stability while continuing aggressive pending→curated→score expansion from the post-`shmctl02` state.
```

Expected: the runbook reflects the approved strategy for the next wave.

- [ ] **Step 3: Update the next validation path if the measured commands or ordering changed**

If Task 4 used the same commands successfully, keep the command block unchanged. If the new evidence requires a narrower verification sequence, replace the relevant lines in section `5.2` with the exact commands you actually used, for example:

```md
TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv-pending
TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv-curated
TIMEOUT_SECS=2400 WHUSE_LTP_PROFILE=score WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv
```

Expected: section `5.2` stays executable and matches reality.

- [ ] **Step 4: Run a focused diff review for the documentation update**

```bash
git diff -- AGENTS.md
```

Expected: shows only the intended state update and strategy wording changes.

- [ ] **Step 5: Commit the runbook refresh**

```bash
git add AGENTS.md
git commit -m "docs(AGENTS): refresh glibc-rv ltp state"
```

Expected: commit contains only the runbook update.

---

## Plan Self-Review

### Spec coverage

- **glibc-rv as the sole priority lane:** covered by Tasks 1, 4, and 5, all of which operate only on glibc-rv promotion files and documentation.
- **Aggressive pending→curated→score promotion:** covered by Task 1 and Task 4 with explicit pending, curated, and score commands.
- **Targeted `shmctl02` fix only:** covered by Tasks 2 and 3, scoped to `crates/syscall/src/lib.rs`.
- **Avoid broad refactors / unrelated work:** enforced by each task’s file list and the absence of unrelated files.
- **Update runbook state:** covered by Task 5.

No spec gaps found.

### Placeholder scan

- No `TBD`, `TODO`, or “similar to Task N” placeholders remain.
- Every code-changing step includes explicit code blocks.
- Every execution step includes explicit commands and expected outcomes.

### Type consistency

- `ShmidDs`, `ShmSegment`, `IPC_SET`, `IPC_STAT`, and `IPC_RMID` are named consistently across Tasks 2 and 3.
- The promotion commands consistently use `WHUSE_OSCOMP_RUNTIME_FILTER=glibc` for glibc-rv-specific runs.
- `tools/oscomp/ltp/score_whitelist_glibc_rv.txt` is the only score file named for glibc-rv promotion.
