# LoongArch BusyBox Bring-up Design

## Summary

This design advances LoongArch BusyBox support in three phases aligned with the user's priority order:

1. First, bring up the BusyBox execution path on LoongArch for both musl and glibc.
2. Next, fix syscall and VFS semantics that block BusyBox applets and scripts.
3. Finally, align the BusyBox path with the OSComp scoring flow so logs, timeouts, and runtime behavior are stable and scoreable.

The near-term success criterion is stronger than simple control-plane reachability: both `/musl/busybox_testcode.sh` and `/glibc/busybox_testcode.sh` should execute end-to-end on LoongArch, and core applets should mostly succeed rather than only producing markers.

## Current project context

The repository already contains a complete LoongArch OSComp path and explicitly wires BusyBox into the LoongArch runtime:

- `crates/kernel-core/src/lib_loongarch.inc.rs` lists both `/musl/busybox` and `/glibc/busybox` as required test files.
- The same file preloads BusyBox test scripts and command files for both runtimes.
- LoongArch has BusyBox-specific timeout configuration (`OSCOMP_BUSYBOX_APPLET_TIMEOUT_NS`, `OSCOMP_BUSYBOX_SUPERVISOR_TIMEOUT_NS`) and a dedicated runtime-dispatch path.
- Recent work has already landed runtime dispatch timeout support for LoongArch (`b428c5e`), so the path is active rather than missing.

This means the project is not starting from zero. The likely gaps are now concentrated in execution completeness, syscall/VFS correctness, and LoongArch-specific runtime stability.

## Goal

Advance LoongArch BusyBox support from “wired into the test harness” to “usable and score-aligned” by ensuring:

- both musl and glibc BusyBox paths can be entered reliably;
- BusyBox shell and common applets run with mostly correct behavior;
- BusyBox test scripts complete without hangs or premature supervisor termination;
- OSComp BusyBox logging and timeout behavior remain stable enough for repeated scoring runs.

## Non-goals

This design does not attempt to:

- broadly expand unrelated LoongArch feature coverage outside BusyBox-driven needs;
- fully optimize score in the first iteration before establishing stable execution;
- refactor unrelated kernel subsystems that do not materially affect BusyBox bring-up.

## Recommended approach

### Approach A — recommended: control-plane first, then semantic clusters, then scoring alignment

1. Stabilize the execution chain for `/musl/busybox_testcode.sh` and `/glibc/busybox_testcode.sh`.
2. Use the resulting failure surface to group and fix syscall/VFS blockers by semantic cluster.
3. After script completion is stable, tune logging, timeout, and supervisor behavior for OSComp score runs.

**Why this is recommended:** it directly matches the requested priority order (“先1然后23”), separates “cannot run” from “runs incorrectly,” and minimizes false debugging signals from partially broken control-plane behavior.

### Approach B — applet-by-applet repair

Run BusyBox, collect failures, and repair each failing applet individually.

**Trade-off:** simple to reason about, but inefficient if common control-plane bugs cause broad cascades of secondary failures.

### Approach C — runtime-difference-first convergence

Prioritize musl/glibc divergence handling first (loader, interpreter, dynamic linker path, runtime-specific layout), then repair shared semantics.

**Trade-off:** useful if the main blocker is glibc-specific startup, but too likely to delay progress on shared BusyBox semantics.

## Design

### 1. Execution layers

#### Layer A: BusyBox bring-up / execution closure

Purpose: ensure the LoongArch BusyBox path can execute from script entry through shell exit for both runtimes.

Responsibilities:

- resolve script shebangs correctly;
- execute BusyBox ELF images from `/musl/busybox` and `/glibc/busybox`;
- preserve correct `argv`/`envp` behavior when dispatching to shell and applets;
- avoid hangs, premature timeout kills, or supervisor dead-ends during BusyBox-driven execution.

Primary implementation areas:

- `crates/kernel-core/src/lib_loongarch.inc.rs`
- `crates/syscall/src/lib.rs`
- `crates/vfs/src/lib.rs` only if shebang, interpreter, or path lookup debugging proves a VFS/path contract issue

#### Layer B: syscall/VFS semantic support

Purpose: make the BusyBox shell and common applets behave correctly enough that scripts mostly succeed instead of only launching.

Likely priority clusters:

1. process lifecycle: `fork`/`clone`, `execve`, `wait*`
2. filesystem operations: `open`/`openat`, `close`, `read`, `write`, `stat*`, `getdents*`, `access`, `readlink`
3. fd plumbing: `pipe`, `dup`, `dup2`, `fcntl`
4. path/cwd behavior: `chdir`, `getcwd`, mount visibility, interpreter lookup

Primary implementation areas:

- `crates/syscall/src/lib.rs`
- `crates/vfs/src/lib.rs`

#### Layer C: LoongArch runtime stability

Purpose: isolate architecture-specific issues that can make BusyBox appear semantically broken even when syscall logic is mostly correct.

Areas to watch:

- timer/interrupt cadence;
- trap return and preemption interaction;
- watchdog interaction with longer-running BusyBox shell/script control flow;
- HAL behavior that can destabilize shell-heavy workloads.

Primary implementation areas:

- `crates/hal-loongarch64-virt/src/lib.rs`
- `crates/kernel-core/src/lib_loongarch.inc.rs`

### 2. Execution flow to validate

The implementation and debugging flow should treat BusyBox execution as three linked stages.

#### Stage 1: script and interpreter entry

`busybox_testcode.sh` → shebang resolution → BusyBox shell launch

Validation points:

- script interpreter path resolves correctly;
- musl BusyBox executes as a static or expected musl binary;
- glibc BusyBox executes with the required LoongArch loader and shared libraries present;
- `execve` correctly distinguishes ELF execution from shell-script interpreter dispatch.

#### Stage 2: shell and applet execution

BusyBox shell → core applets (`echo`, `true`, `ls`, `cat`, `mkdir`, `rm`, `sh`) → shell regains control

Validation points:

- basic fd inheritance for stdin/stdout/stderr;
- correct `cwd` handling and relative path behavior;
- directory iteration and metadata queries work well enough for shell scripts and `ls`-like applets;
- pipe and redirection behavior is sufficiently correct for shell control flow.

#### Stage 3: OSComp integration

test script → group markers / step markers / timeout behavior → suite completion

Validation points:

- BusyBox workloads do not get killed spuriously by LoongArch watchdog logic;
- BusyBox groups emit consistent markers and termination signals;
- both musl and glibc BusyBox paths can be rerun stably in OSComp-oriented environments.

### 3. Implementation sequencing

#### Phase 1: establish the current failure map

Before broad fixes, collect the LoongArch BusyBox failure surface for both runtimes and classify each failure into one of three buckets:

1. startup/control-plane failure (cannot enter BusyBox or shell reliably)
2. semantic failure (BusyBox runs, but applets/scripts fail due to syscall/VFS gaps)
3. scoring/runtime integration failure (BusyBox mostly works, but OSComp markers, timeouts, or completion behavior are unstable)

This classification prevents mixing unrelated causes and keeps changes minimal.

#### Phase 2: close the shortest execution path

Target outcome:

- `/musl/busybox_testcode.sh` completes;
- `/glibc/busybox_testcode.sh` completes;
- core applets mostly succeed.

Priority:

- eliminate hangs, startup crashes, and non-returning supervisor paths first;
- then restore shell/app applet viability;
- only after this, widen coverage to lower-value applet failures.

#### Phase 3: repair semantic clusters in dependency order

Use the smallest set of fixes that unlock the largest BusyBox surface area. The preferred repair order is:

1. `execve` / process creation / waiting
2. filesystem lookup and metadata operations
3. fd duplication / pipe / redirection
4. cwd, mount, and path-resolution behavior
5. residual syscall gaps exposed by retesting

This order reflects how shell-heavy BusyBox flows fail in practice: process and path handling usually dominate before more specialized syscalls matter.

#### Phase 4: align OSComp scoring behavior

Once both BusyBox scripts execute stably, tighten score-path integration:

- confirm BusyBox group execution emits the markers expected by the OSComp parsing flow;
- confirm timeout values are appropriate for LoongArch BusyBox applets and supervisor processes;
- confirm repeated runs do not regress due to watchdog drift, logging mismatches, or runtime-dispatch edge cases.

## Error-handling strategy

The implementation should stay narrow and diagnostic-first:

- do not expand syscall coverage speculatively;
- add only focused observability around the BusyBox path when needed;
- keep musl/glibc shared fixes unified, but isolate runtime-specific startup issues when they differ;
- treat LoongArch timer/trap/HAL issues as a separate lane from generic syscall semantics.

## Testing strategy

Validation should happen in three layers.

### Layer 1: minimal BusyBox probes

Use direct probes to prove the shell/app path works before larger scripts are trusted:

- `/musl/busybox sh -c 'echo ok'`
- `/musl/busybox sh -c 'true; echo done'`
- `/musl/busybox ls /`
- `/glibc/busybox sh -c 'echo ok'`
- `/glibc/busybox sh -c 'true; echo done'`
- `/glibc/busybox ls /`

These commands make the dual-runtime expectation explicit instead of leaving it implicit.

### Layer 2: dual-runtime script closure

Run:

- `/musl/busybox_testcode.sh`
- `/glibc/busybox_testcode.sh`

Success criterion for this phase:

- both scripts execute end-to-end on LoongArch;
- core applets mostly succeed;
- failures are concentrated into a manageable semantic backlog rather than hangs or total startup failure.

### Layer 3: OSComp-aligned validation

Run the LoongArch BusyBox path in host/contest-aligned environments and verify:

- group markers and step markers are present and well-formed;
- timeout handling is stable;
- repeated runs are reproducible enough for scoring work.

## Acceptance criteria

The design is considered satisfied when all of the following are true:

1. LoongArch can execute both `/musl/busybox_testcode.sh` and `/glibc/busybox_testcode.sh` end-to-end.
2. Core BusyBox applets mostly succeed instead of failing wholesale.
3. Remaining failures are traceable to bounded semantic gaps rather than control-plane instability.
4. The BusyBox OSComp path emits stable completion/logging behavior suitable for subsequent score improvement work.

## Files expected to change during implementation

Most work should stay concentrated in the currently relevant paths:

- `crates/kernel-core/src/lib_loongarch.inc.rs`
- `crates/syscall/src/lib.rs`
- `crates/vfs/src/lib.rs`
- `crates/hal-loongarch64-virt/src/lib.rs`

Additional supporting changes may be required only if the existing BusyBox scripts or stage2 runner logic expose a contract mismatch.
