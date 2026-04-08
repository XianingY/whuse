# glibc-rv LTP Aggressive Expansion Design

## Summary

This design advances the repository's current RISC-V LTP work by making **glibc-rv** the sole decision-driving lane for the next push. The strategy is to first maximize gains from the existing `pending -> curated -> score` pipeline, then unlock additional throughput with a tightly-scoped SysV IPC semantic fix centered on `shmctl02`.

The design explicitly avoids broad syscall refactors, cross-architecture expansion, and unrelated runner cleanups. It is meant to grow score-bearing glibc-rv coverage quickly without regressing the current score whitelist.

## Goal

Increase **glibc-rv** LTP score coverage as aggressively as possible without removing or destabilizing already-scoring cases, using:

1. fast promotion of already-healthy glibc-rv cases through the whitelist pipeline, and
2. one targeted semantic repair for the highest-value SysV IPC blocker: `shmctl02`.

## Current Context

Current repository state already contains the needed control-plane and promotion machinery:

- `AGENTS.md` records the current public baseline as `cc91617` with `glibc-rv=1088` and `musl-rv=1782`.
- `tools/dev/run_oscomp_stage2.sh` already supports separate pending / curated / score flows and runtime filtering.
- glibc-rv pending cases currently include `fcntl17_64`, `ftruncate04_64`, `shmctl02`, `openat04`, `prctl06`, `preadv203`, `utime02`, and `pipe04`.
- `shmctl02` is already documented as unresolved and intentionally excluded from score promotion.
- `crates/syscall/src/lib.rs` currently implements `sys_shmctl`, but only handles `IPC_RMID` and `IPC_STAT`; `IPC_SET` is not implemented in `sys_shmctl` even though related message-queue control code supports more commands.

This means the next gains are likely to come from a combination of list management and one small semantic correction rather than from rebuilding the LTP pipeline.

## Recommended Approach

### Approach 1: glibc-rv promotion-first plus targeted `shmctl02` fix (recommended)

Use the existing glibc-rv pending / curated / score pipeline as the main throughput engine. Run glibc-rv promotion waves aggressively, inspect failures, and only stop to fix one blocker with clear score impact: `shmctl02`.

**Why this is recommended:**
- It matches the user's stated priority order exactly.
- It converts already-working cases into score quickly.
- It limits semantic risk by keeping the syscall work narrow.

### Approach 2: pure promotion-first pass, no kernel changes initially

Run only the glibc-rv whitelist pipeline and defer all syscall work until promotion naturally stalls.

**Trade-off:** fastest short-term feedback, but likely to hit a ceiling quickly if `shmctl02` or related SysV semantics are blocking the remaining high-value cases.

### Approach 3: SysV IPC repair before more promotion

Deep-dive `shmctl02` first, possibly broadening into surrounding shared-memory semantics before any additional promotion waves.

**Trade-off:** strongest semantic cleanup, but slowest path to immediate score growth and highest chance of scope drift.

## Selected Design

The selected design is **Approach 1**.

### Architecture

The work is split into two tightly-bounded tracks:

#### Track A: glibc-rv whitelist throughput

Use `tools/dev/run_oscomp_stage2.sh` and the glibc-rv LTP whitelist files to push cases through three layers:

- **pending**: discovery layer; failures are allowed
- **curated**: stability layer; `bad=0 conf=0` required
- **score**: scorer-visible production layer; must not regress

The purpose of this track is to convert naturally-passing or near-passing glibc-rv cases into score as quickly as possible.

#### Track B: targeted SysV IPC semantic unlock

Focus semantic work only on `sys_shmctl` and only as needed to make `shmctl02` promotable. This includes validating behavior for:

- `IPC_STAT`
- `IPC_SET`
- `IPC_RMID`
- shared-memory metadata updates and removal semantics relevant to the LTP case

The purpose of this track is not general SysV IPC completeness. It is to remove a known blocker that is already preventing promotion.

### Component Boundaries

#### LTP list management

Files:
- `tools/oscomp/ltp/pending_whitelist_glibc_rv.txt`
- `tools/oscomp/ltp/pending_blacklist_glibc_rv.txt`
- `tools/oscomp/ltp/curated_whitelist_glibc_rv.txt`
- `tools/oscomp/ltp/curated_blacklist_glibc_rv.txt`
- `tools/oscomp/ltp/score_whitelist_glibc_rv.txt`
- `tools/oscomp/ltp/score_blacklist_glibc_rv.txt`

Responsibility:
- define which glibc-rv cases are under evaluation,
- identify stability-proven cases,
- preserve the scorer-safe layer.

No semantic fixes belong here. These files only express promotion state.

#### Stage2 orchestration

File:
- `tools/dev/run_oscomp_stage2.sh`

Responsibility:
- execute pending / curated / score runs,
- separate runtime lanes cleanly,
- expose enough output to classify promotion candidates vs real semantic failures.

Changes here should only be made if promotion logic or result classification is blocking glibc-rv expansion. No broad shell refactor is in scope.

#### RISC-V LTP helper generation

File:
- `crates/kernel-core/src/lib_riscv.inc.rs`

Responsibility:
- generate or wire LTP execution helpers,
- preserve scorer-visible output markers,
- ensure glibc-rv LTP cases actually run and remain visible to the judge.

Changes here should be control-plane only if needed. No unrelated ordering or platform work is included.

#### SysV shared-memory control semantics

File:
- `crates/syscall/src/lib.rs`

Responsibility:
- implement `sys_shmctl` with the minimum semantics required for `shmctl02` to pass,
- preserve existing shared-memory attach/detach behavior,
- keep destruction and metadata updates consistent.

If necessary, small supporting changes in related IPC code are allowed, but only when they are directly required by `shmctl02`.

## Execution Flow

1. Run **glibc-rv pending** to identify pass / fail / unstable cases.
2. Promote any stable pass candidates into **curated**.
3. Run **glibc-rv curated** and require `bad=0 conf=0`.
4. Promote a bounded batch from curated into **score**.
5. Inspect the remaining failures; if `shmctl02` is still failing, isolate that failure mode.
6. Fix `sys_shmctl` narrowly for `shmctl02`.
7. Re-run the glibc-rv pending / curated / score flow.
8. Update `AGENTS.md` with the new glibc-rv state and blocker status.

The key rule is: **list management drives throughput, syscall work unlocks the next plateau**.

## Error Handling and Risk Control

### Non-goals

This design explicitly does **not** include:

- LoongArch expansion
- large musl-rv sync work
- broad SysV IPC redesign
- fixing every pending glibc-rv blocker in one batch
- unrelated cleanup in the runner or kernel

### Risk controls

- Never remove an existing glibc-rv score-whitelisted case unless it is proven to be invalid and the user explicitly approves a rollback.
- Treat curated failures as a hard gate; do not promote through `bad` or `conf` noise.
- Keep `shmctl02` work localized to `sys_shmctl` and directly-related shared-memory state.
- If promotion gains continue without syscall changes, prefer more promotion before more semantic edits.
- If `shmctl02` reveals a deeper object-lifetime bug, stop at the smallest fix that makes the case stable rather than redesigning the whole IPC subsystem.

## Testing Strategy

### Promotion validation

Primary validation for this design is glibc-rv-specific:

```bash
TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv-pending
TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv-curated
TIMEOUT_SECS=2400 WHUSE_LTP_PROFILE=score tools/dev/run_oscomp_stage2.sh ltp-riscv
```

Expected outcomes:
- pending reveals promotable candidates or focused failures,
- curated remains `bad=0 conf=0`,
- score whitelist grows monotonically,
- existing score cases remain stable.

### Control-plane regression check

```bash
bash tools/dev/test_run_oscomp_stage2.sh
```

Expected outcome:
- stage2 orchestration remains healthy after any runner-side changes.

### Targeted syscall verification

After `shmctl02` work:
- rerun the glibc-rv pending / curated / score flow listed above,
- confirm `shmctl02` either reaches the curated-promotion bar (`bad=0 conf=0` in the glibc-rv lane) or fails with a single, directly identified residual semantic gap,
- if implementation adds focused `sys_shmctl` tests in `crates/syscall/src/lib.rs`, treat them as supplemental checks rather than the release gate.

## Success Criteria

This design is successful when:

1. glibc-rv remains the sole prioritized lane for the current expansion batch,
2. at least one promotion wave is completed without score regression,
3. `shmctl02` is either fixed or reduced to a sharply-scoped residual issue with direct code evidence,
4. `AGENTS.md` reflects the new glibc-rv status and remaining blocker set.

## Testing and Design Notes

Implementation should keep promotion-only list updates separate from `sys_shmctl` semantic changes unless a checkpoint run shows they must land together to keep the glibc-rv lane coherent.
