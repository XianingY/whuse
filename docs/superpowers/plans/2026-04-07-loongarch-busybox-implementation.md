# LoongArch BusyBox Bring-up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make LoongArch execute both `/musl/busybox_testcode.sh` and `/glibc/busybox_testcode.sh` end-to-end, then close the highest-yield syscall/VFS gaps and stabilize the OSComp BusyBox score path.

**Architecture:** Keep the work concentrated in the existing LoongArch OSComp pipeline. First harden the dual-runtime BusyBox dispatch path in `kernel-core`, then repair `execve`/BusyBox applet dispatch and the most leveraged syscall/VFS semantics in `syscall` and `vfs`, and only then tune LoongArch runtime/watchdog behavior if the scripts still hang or terminate spuriously.

**Tech Stack:** Rust (`kernel-core`, `syscall`, `vfs`, `hal-loongarch64-virt`), BusyBox shell scripts, QEMU LoongArch64, existing `make` / `xtask` OSComp runners

---

## File Structure

| File | Responsibility |
|------|---------------|
| `crates/kernel-core/src/lib_loongarch.inc.rs` | LoongArch OSComp suite generation, dual-runtime dispatch, BusyBox step orchestration, timeout/watchdog wiring |
| `crates/syscall/src/lib.rs` | `execve`, BusyBox applet detection, process/wait behavior, syscall debug markers, per-applet probes |
| `crates/syscall/src/task_domain.rs` | Task-domain routing for clone/exec/wait entrypoints (read for validation only unless routing bug found) |
| `crates/syscall/src/fs_domain.rs` | Filesystem syscall routing (`openat`, `getdents64`, `readlinkat`, `chdir`, `getcwd`) (read for validation only unless dispatch bug found) |
| `crates/vfs/src/lib.rs` | Path normalization, open/create behavior, symlink resolution, directory/stat semantics used by BusyBox shell/applets |
| `crates/hal-loongarch64-virt/src/lib.rs` | LoongArch timer/interrupt behavior if BusyBox still hangs after syscall/VFS closure |
| `docs/superpowers/specs/2026-04-07-loongarch-busybox-design.md` | Approved design input for this plan |

---

### Task 1: Capture the LoongArch BusyBox failure map

**Files:**
- Modify: `crates/kernel-core/src/lib_loongarch.inc.rs:804-888`
- Modify: `crates/syscall/src/lib.rs:3745-3926`
- Test: use existing LoongArch OSComp runner commands only

- [ ] **Step 1: Add explicit dual-runtime BusyBox probe markers before script execution**

Add minimal markers inside `run_runtime_script_step()` so each BusyBox runtime prints the exact shell command it is about to execute.

```rust
const OSCOMP_SUITE_SCRIPT_REAL_FULL_TEMPLATE: &str = concat!(
    // existing content ...
    "    echo whuse-oscomp-runtime-begin:$runtime\n",
    "    echo whuse-oscomp-runtime-probe:$runtime:root=$root:script=$script:timeout=$timeout_s\n",
    "    case \"$script\" in\n",
    "    basic_testcode.sh)\n",
    "        run_basic_testsuite_runtime_entry \"$runtime\" \"$timeout_s\"\n",
    "        rc=$?\n",
    "        ;;\n",
    "    *)\n",
    "        echo whuse-oscomp-runtime-exec:$runtime:/musl/busybox sh -c cd\ $root\ \&\&\ ./$script\n",
    "        if [ \"$WHUSE_HAS_TIMEOUT\" = \"1\" ]; then\n",
    "            /musl/busybox timeout \"$timeout_s\" /musl/busybox sh -c \"cd $root && ./$script\"\n",
    "        else\n",
    "            /musl/busybox sh -c \"cd $root && ./$script\"\n",
    "        fi\n",
    "        rc=$?\n",
    "        if [ \"$rc\" = \"124\" ]; then\n",
    "            echo whuse-oscomp-step-timeout:${runtime}/$script:$timeout_s:pid=0:tgid=0\n",
    "        fi\n",
    "        ;;\n",
    "    esac\n",
    // existing content ...
);
```

- [ ] **Step 2: Add an `execve` probe only for BusyBox shell/script entry**

Insert a narrow debug branch near `sys_execve()` so the logs show path, cwd, argv, and the final redirected display path for BusyBox-related launches only.

```rust
let busybox_exec_probe = display_path.contains("busybox")
    || path.contains("busybox")
    || argv.first().is_some_and(|arg| arg.contains("busybox"))
    || path.ends_with("busybox_testcode.sh");
if busybox_exec_probe {
    log_always(&format!(
        "whuse-busybox-exec:cwd={} raw_path={} display_path={} argv={:?}",
        cwd, path, display_path, argv
    ));
}
```

- [ ] **Step 3: Build the LoongArch kernel with the probe changes**

Run:
```bash
make -C /home/wslootie/github/whuse oscomp-loongarch-host
```
Expected: build completes and produces updated `kernel-la` with no Rust compile errors.

- [ ] **Step 4: Run only the BusyBox step to capture the musl/glibc failure split**

Run:
```bash
WHUSE_OSCOMP_PROFILE=full WHUSE_OSCOMP_ONLY_STEP=busybox_testcode.sh make -C /home/wslootie/github/whuse oscomp-loongarch-host
```
Expected: log contains `whuse-oscomp-runtime-begin:musl`, `whuse-oscomp-runtime-begin:glibc`, `whuse-oscomp-runtime-probe:*`, `whuse-busybox-exec:*`, and a final `whuse-oscomp-step-end:busybox_testcode.sh:<rc>`.

- [ ] **Step 5: Classify the result before editing semantics**

Use the captured log and assign the current failure to one of these buckets in your working notes before changing code further:

```text
1. startup/control-plane: runtime step cannot enter busybox/shell
2. semantic: script enters busybox but applets/syscalls fail
3. runtime integration: script mostly works but times out or exits incorrectly
```

Expected: one dominant bucket for musl and one for glibc.

- [ ] **Step 6: Commit the probe-only checkpoint**

```bash
git add crates/kernel-core/src/lib_loongarch.inc.rs crates/syscall/src/lib.rs
git commit -m "debug(loongarch): trace dual-runtime busybox bring-up"
```

---

### Task 2: Make dual-runtime BusyBox script entry reliable

**Files:**
- Modify: `crates/kernel-core/src/lib_loongarch.inc.rs:804-888`
- Modify: `crates/syscall/src/lib.rs:3745-3926`
- Test: existing LoongArch host runner

- [ ] **Step 1: Write the failing expectation for runtime entry**

The target behavior for this task is:

```text
musl runtime emits whuse-oscomp-runtime-end:musl after busybox_testcode.sh
glibc runtime emits whuse-oscomp-runtime-end:glibc after busybox_testcode.sh
root step emits whuse-oscomp-step-end:busybox_testcode.sh:<non-timeout rc>
```

Use this exact command as the failure gate:

```bash
WHUSE_OSCOMP_PROFILE=full WHUSE_OSCOMP_ONLY_STEP=busybox_testcode.sh make -C /home/wslootie/github/whuse oscomp-loongarch-host
```

Expected before fix: at least one runtime fails to reach its `whuse-oscomp-runtime-end:*` marker or exits through the wrong BusyBox path.

- [ ] **Step 2: Make the runtime shell use the runtime-local BusyBox binary instead of hard-coding `/musl/busybox`**

Change `run_runtime_script_step()` so each runtime invokes its own BusyBox binary first, while still using the existing timeout path when enabled.

```rust
"    busybox_bin=\"$root/busybox\"\n",
"    if [ \"$WHUSE_HAS_TIMEOUT\" = \"1\" ]; then\n",
"        /musl/busybox timeout \"$timeout_s\" \"$busybox_bin\" sh -c \"cd $root && ./$script\"\n",
"    else\n",
"        \"$busybox_bin\" sh -c \"cd $root && ./$script\"\n",
"    fi\n",
```

This keeps the timeout helper stable while removing the current runtime asymmetry in the actual shell executor.

- [ ] **Step 3: Preserve the BusyBox applet name after shell redirection in `sys_execve()`**

Keep the existing applet tracking logic, but make sure `display_path` and `argv` remain aligned after any shell redirection so `BUSYBOX_APPLETS` records the actual applet/shell name used for the task.

```rust
let busybox_applet = if display_path.contains("busybox") && argv.len() > 1 {
    Some(argv[1].clone())
} else if matches!(display_path.as_str(), "/bin/sh" | "/bin/bash" | "/busybox") {
    Some("sh".to_string())
} else {
    None
};
if let Some(applet) = busybox_applet {
    BUSYBOX_APPLETS
        .lock()
        .insert(procs.current_tgid().unwrap_or(0), applet.clone());
    log_always(&format!(
        "whuse-busybox-applet:tgid={} applet={} path={}",
        procs.current_tgid().unwrap_or(0),
        applet,
        display_path
    ));
}
```

- [ ] **Step 4: Rebuild and rerun the BusyBox-only LoongArch step**

Run:
```bash
make -C /home/wslootie/github/whuse oscomp-loongarch-host && WHUSE_OSCOMP_PROFILE=full WHUSE_OSCOMP_ONLY_STEP=busybox_testcode.sh make -C /home/wslootie/github/whuse oscomp-loongarch-host
```
Expected: both runtimes enter via their own `/$runtime/busybox`, print runtime begin/end markers, and no longer fail due to the shell being forced through the musl binary.

- [ ] **Step 5: Commit the runtime-entry fix**

```bash
git add crates/kernel-core/src/lib_loongarch.inc.rs crates/syscall/src/lib.rs
git commit -m "fix(loongarch): use per-runtime busybox shell entry"
```

---

### Task 3: Close the highest-yield syscall and VFS gaps for BusyBox applets

**Files:**
- Modify: `crates/syscall/src/lib.rs:3745-3926`
- Modify: `crates/vfs/src/lib.rs:753-1222`
- Test: BusyBox minimal probes and BusyBox step reruns

- [ ] **Step 1: Write the failing probe list for core applets**

Use these concrete probes as the regression gate for this task:

```text
/musl/busybox sh -c 'echo ok'
/musl/busybox sh -c 'true; echo done'
/musl/busybox ls /
/glibc/busybox sh -c 'echo ok'
/glibc/busybox sh -c 'true; echo done'
/glibc/busybox ls /
```

Run them through the existing BusyBox script path by reusing `busybox_testcode.sh`; do not create a new harness.

Command:
```bash
WHUSE_OSCOMP_PROFILE=full WHUSE_OSCOMP_ONLY_STEP=busybox_testcode.sh make -C /home/wslootie/github/whuse oscomp-loongarch-host
```
Expected before fix: at least one of `echo`, `true`, `pwd`, `which`, `touch`, or `ls` fails consistently in one or both runtimes.

- [ ] **Step 2: Remove LoongArch-only noisy `open_with_owner()` tracing once the initial failure map is known**

The current unconditional LoongArch prints in `open_with_owner()` will drown out BusyBox debugging. Gate them behind the existing stage2 flag.

```rust
if stage2_openat_debug_enabled() && cfg!(target_arch = "loongarch64") {
    write_console_line(&format!(
        "whuse-la-basic:vfs-open-with-owner-start cwd={} path={} absolute={} flags={:#x}",
        cwd, path, absolute, flags
    ));
}
```

Apply the same guard to the matching `before-open` and `after-open` logs in the same function.

- [ ] **Step 3: Fix BusyBox path and symlink resolution behavior in `vfs::open()` / `open_mem()` if the probe log shows shell-script or applet lookup drift**

Use the existing `normalize_path()` + symlink-follow path, but make sure shell-executed relative script paths stay rooted under the runtime directory after `cd $root`.

```rust
pub fn open(&mut self, cwd: &str, path: &str, flags: u32, mode: u32) -> KernelResult<FileHandle> {
    let absolute = normalize_path(cwd, path);
    // keep using absolute for all later lookups
    if let Some(handle) = self.try_open_external(&absolute, flags)? {
        return Ok(handle);
    }
    self.open_mem(&absolute, flags, mode)
}
```

Expected result: `./busybox_testcode.sh`, `./busybox_cmd.txt`, and applet-related relative paths resolve under `/musl` or `/glibc` instead of drifting to `/`.

- [ ] **Step 4: Fix the first confirmed semantic blocker in `sys_execve`, wait, or filesystem handling before broadening scope**

Pick the first blocker revealed by Task 2 logs and make the minimal fix. Use one of the following patterns depending on the observed failure:

```rust
// if argv/display_path mismatch breaks busybox applet dispatch
if display_path.ends_with("/busybox") && argv.is_empty() {
    argv.push(display_path.clone());
}
```

```rust
// if script execution passes an empty interpreter/script arg boundary
if display_path.ends_with("/busybox") && argv.len() == 1 {
    argv.push("sh".to_string());
}
```

```rust
// if cwd-sensitive file lookup is the blocker
let path = if path.is_empty() { "." } else { path };
```

Expected: one concrete BusyBox failure disappears entirely after this step.

- [ ] **Step 5: Rerun the BusyBox-only LoongArch step and verify core applet improvement**

Run:
```bash
WHUSE_OSCOMP_PROFILE=full WHUSE_OSCOMP_ONLY_STEP=busybox_testcode.sh make -C /home/wslootie/github/whuse oscomp-loongarch-host
```
Expected: both runtimes complete the BusyBox script, and the log shows materially fewer failures for `true`, `echo`, `which`, `uname`, `pwd`, or `touch` than before the semantic fix.

- [ ] **Step 6: Commit the semantic closure wave**

```bash
git add crates/syscall/src/lib.rs crates/vfs/src/lib.rs
git commit -m "fix(loongarch): unblock core busybox applet semantics"
```

---

### Task 4: Stabilize LoongArch runtime/watchdog behavior only if BusyBox still hangs or times out

**Files:**
- Modify: `crates/hal-loongarch64-virt/src/lib.rs:142-173`
- Modify: `crates/kernel-core/src/lib_loongarch.inc.rs:400-545, 780-958`
- Test: BusyBox-only rerun, then full LoongArch host run

- [ ] **Step 1: Confirm the failure is really runtime/watchdog-related before touching HAL**

Run:
```bash
WHUSE_OSCOMP_PROFILE=full WHUSE_OSCOMP_ONLY_STEP=busybox_testcode.sh make -C /home/wslootie/github/whuse oscomp-loongarch-host
```
Expected before HAL changes: logs show `whuse-oscomp-step-timeout:*`, missing `whuse-oscomp-runtime-end:*`, or repeated timer/watchdog symptoms after the syscall/VFS fixes are already in place.

- [ ] **Step 2: Add a bounded LoongArch timer interrupt heartbeat around the kernel trap handler**

Instrument the timer path without flooding logs.

```rust
static TIMER_HEARTBEAT_BUDGET: AtomicUsize = AtomicUsize::new(64);

unsafe extern "C" fn __whuse_kernel_trap_handler(estat: usize, era: usize) {
    let is_timer = (estat & ECFG_TI) != 0;
    if is_timer {
        if TIMER_HEARTBEAT_BUDGET
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
            .is_ok()
        {
            let mut console = hal_api::ConsoleWriter;
            let _ = core::fmt::Write::write_fmt(
                &mut console,
                format_args!("whuse-la-timer-heartbeat estat={:#x} era={:#x}\n", estat, era),
            );
        }
        core::arch::asm!("li.d $t0, 1", "csrwr $t0, 0x44", out("$t0") _);
        let cb_ptr = KERNEL_TRAP_HANDLER.load(core::sync::atomic::Ordering::Relaxed);
        if cb_ptr != 0 {
            let cb: fn() = core::mem::transmute(cb_ptr);
            cb();
        }
        return;
    }
    // existing fatal path
}
```

- [ ] **Step 3: Relax only the BusyBox timeout path if the log proves the supervisor kills good work**

Adjust the BusyBox dual-runtime timeout in the real full template, not the whole suite.

```rust
"    run_runtime_dual_step busybox_testcode.sh 600\n",
```

Make this change only if the previous log shows genuine long-running progress before timeout; do not widen unrelated step timeouts.

- [ ] **Step 4: Rebuild and rerun BusyBox-only, then full host LoongArch**

Run:
```bash
make -C /home/wslootie/github/whuse oscomp-loongarch-host && WHUSE_OSCOMP_PROFILE=full WHUSE_OSCOMP_ONLY_STEP=busybox_testcode.sh make -C /home/wslootie/github/whuse oscomp-loongarch-host && make -C /home/wslootie/github/whuse oscomp-loongarch-host
```
Expected: BusyBox no longer times out spuriously, timer heartbeat shows continued interrupts if needed, and the full host run reaches `whuse-oscomp-suite-done` or at least clears BusyBox cleanly before later steps.

- [ ] **Step 5: Commit the LoongArch stability fix if one was needed**

```bash
git add crates/hal-loongarch64-virt/src/lib.rs crates/kernel-core/src/lib_loongarch.inc.rs
git commit -m "fix(loongarch): stabilize busybox runtime watchdog path"
```

---

### Task 5: Validate OSComp-aligned BusyBox closure for both runtimes

**Files:**
- Modify: `crates/kernel-core/src/lib_loongarch.inc.rs` only if marker contract mismatch remains
- Test: host run and contest-aligned run

- [ ] **Step 1: Run the final BusyBox-focused host validation**

Run:
```bash
WHUSE_OSCOMP_PROFILE=full WHUSE_OSCOMP_ONLY_STEP=busybox_testcode.sh make -C /home/wslootie/github/whuse oscomp-loongarch-host
```
Expected: log contains all of:

```text
whuse-oscomp-step-begin:busybox_testcode.sh
whuse-oscomp-runtime-begin:musl
whuse-oscomp-runtime-end:musl
whuse-oscomp-runtime-begin:glibc
whuse-oscomp-runtime-end:glibc
whuse-oscomp-step-end:busybox_testcode.sh:<non-124 rc>
```

- [ ] **Step 2: Run the full LoongArch host validation**

Run:
```bash
make -C /home/wslootie/github/whuse oscomp-loongarch-host
```
Expected: the suite advances beyond BusyBox with no BusyBox-specific hang or early abort.

- [ ] **Step 3: Run the contest-aligned LoongArch validation**

Run:
```bash
make -C /home/wslootie/github/whuse oscomp-loongarch-contest
```
Expected: the contest-aligned path shows the same BusyBox runtime closure markers and does not regress due to container/runtime differences.

- [ ] **Step 4: If marker shape still mismatches score expectations, make the smallest marker-only fix**

Allowed fix pattern in `lib_loongarch.inc.rs`:

```rust
"echo whuse-oscomp-step-begin:busybox_testcode.sh\n",
"run_runtime_dual_step busybox_testcode.sh 300\n",
"rc=$?\n",
"echo whuse-oscomp-step-end:busybox_testcode.sh:$rc\n",
```

Do not rewrite the BusyBox script body here; only fix root-step and runtime-step marker consistency.

- [ ] **Step 5: Commit the final BusyBox closure checkpoint**

```bash
git add crates/kernel-core/src/lib_loongarch.inc.rs crates/syscall/src/lib.rs crates/vfs/src/lib.rs crates/hal-loongarch64-virt/src/lib.rs
git commit -m "feat(loongarch): close dual-runtime busybox oscomp path"
```

---

## Self-Review

### Spec coverage

- Dual-runtime BusyBox bring-up: covered by Tasks 1-2.
- Syscall/VFS semantic closure: covered by Task 3.
- LoongArch runtime stability lane: covered by Task 4.
- OSComp score/log/timeout alignment: covered by Task 5.

No spec requirement is currently uncovered.

### Placeholder scan

- No `TODO`, `TBD`, or deferred unnamed tasks remain.
- All task commands are concrete.
- Every code-changing step includes explicit code snippets.

### Type consistency

- `run_runtime_script_step`, `run_runtime_dual_step`, `sys_execve`, `BUSYBOX_APPLETS`, `open_with_owner`, and `open` match existing names in the current codebase.
- No new helper names are referenced later without being introduced first.
