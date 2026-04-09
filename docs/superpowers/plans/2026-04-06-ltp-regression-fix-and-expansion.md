# Whuse LTP Regression Fix + Incremental Expansion Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the cbb8493 score regression (626.0 vs baseline 2116.0), then incrementally grow musl-rv and glibc-rv LTP scores via 8-16 case promotion waves without regressions.

**Architecture:** Three-phase approach: (1) stop-the-bleeding control-plane fix, (2) LTP pending→curated→score promotion waves, (3) targeted semantic fixes for high-value blockers like shmctl02. Each phase is independently testable and committable.

**Tech Stack:** Rust (kernel-core, syscall), Bash (stage2 runner scripts), LTP test framework, QEMU RISC-V64

---

## File Structure

| File | Responsibility |
|------|---------------|
| `tools/oscomp/ltp/score_whitelist.txt` | RV musl score whitelist (protected) |
| `tools/oscomp/ltp/score_blacklist.txt` | RV musl score blacklist (protected) |
| `tools/oscomp/ltp/score_whitelist_glibc_rv.txt` | RV glibc score whitelist |
| `tools/oscomp/ltp/score_blacklist_glibc_rv.txt` | RV glibc score blacklist |
| `tools/oscomp/ltp/musl_rv_curated_whitelist.txt` | RV musl curated stable layer |
| `tools/oscomp/ltp/musl_rv_curated_blacklist.txt` | RV musl curated exclusions |
| `tools/oscomp/ltp/curated_whitelist_glibc_rv.txt` | RV glibc curated stable layer |
| `tools/oscomp/ltp/pending_whitelist_rv_musl.txt` | RV musl pending work queue |
| `tools/oscomp/ltp/pending_blacklist_rv_musl.txt` | RV musl pending exclusions |
| `tools/oscomp/ltp/pending_whitelist_glibc_rv.txt` | RV glibc pending work queue |
| `tools/oscomp/ltp/pending_blacklist_glibc_rv.txt` | RV glibc pending exclusions |
| `tools/dev/run_oscomp_stage2.sh` | Stage2 runner, promotion orchestration |
| `crates/kernel-core/src/lib_riscv.inc.rs` | RISC-V kernel: syscall dispatch, LTP helper scripts, blocked-restart path |
| `crates/syscall/src/lib.rs` | Syscall implementations (SysV IPC, signals, etc.) |
| `AGENTS.md` | Project runbook — update after each commit |

---

## Phase 1: Regression Stop-Loss (Control Plane Only)

### Task 1: Diagnose and Fix RV LTP Score Regression (1491→73)

**Context:** At cbb8493, musl-rv LTP scored only 73 vs 1491 at baseline. The score whitelist has 306 entries but the site run only scored 73 points. This means either: (a) LTP cases didn't run, (b) output contract wasn't met (missing `RUN LTP CASE`/`FAIL LTP CASE`), or (c) cases failed.

**Files:**
- Read: `tools/dev/run_oscomp_stage2.sh` (LTP profile injection, score path logic)
- Read: `crates/kernel-core/src/lib_riscv.inc.rs` (LTP step script generation, output contract)
- Read: `assist_docs/cbb849355234d6765687d2d0ef5f3514b5f68771/Riscv输出.md` (site log for regression clues)
- Read: `assist_docs/7f5dfcec12826030f0d8ba9020d768bb84c704e1/Riscv输出.md` (baseline log for comparison)
- Modify: kernel or stage2 scripts based on root cause

- [ ] **Step 1: Compare baseline vs regression LTP output contracts**

  Use `strings` + `grep` on both assist_docs files to compare:
  ```bash
  # Baseline (7f5dfce) - count LTP markers
  strings assist_docs/7f5dfcec12826030f0d8ba9020d768bb84c704e1/Riscv输出.md | grep -c "RUN LTP CASE"
  strings assist_docs/7f5dfcec12826030f0d8ba9020d768bb84c704e1/Riscv输出.md | grep -c "FAIL LTP CASE"
  strings assist_docs/7f5dfcec12826030f0d8ba9020d768bb84c704e1/Riscv输出.md | grep -c "whuse-ltp-case-result"

  # Regression (cbb8493)
  strings assist_docs/cbb849355234d6765687d2d0ef5f3514b5f68771/Riscv输出.md | grep -c "RUN LTP CASE"
  strings assist_docs/cbb849355234d6765687d2d0ef5f3514b5f68771/Riscv输出.md | grep -c "FAIL LTP CASE"
  strings assist_docs/cbb849355234d6765687d2d0ef5f3514b5f68771/Riscv输出.md | grep -c "whuse-ltp-case-result"
  ```

  Expected: baseline should show many more RUN/FAIL/case-result lines. If regression shows same count of RUN but fewer FAIL, the issue is output contract. If fewer RUN, the issue is LTP not executing.

- [ ] **Step 2: Identify the LTP profile used at cbb8493**

  Check the cbb8493 Riscv输出.md for `whuse-oscomp-ltp-whitelist-lines` and `whuse-oscomp-ltp-marker:runner-start:profile=`. Compare with baseline.

  Key question: did the site run use `profile=score` or `profile=full`? The AGENTS.md says `WHUSE_LTP_PROFILE=score` by default, but the site may use a different profile.

- [ ] **Step 3: Check if LTP step script was generated correctly**

  In `crates/kernel-core/src/lib_riscv.inc.rs`, find `render_oscomp_ltp_step_helper_script` or equivalent. Check if the LTP testcode.sh generation changed between 7f5dfce and cbb8493:
  ```bash
  git diff 7f5dfce..cbb8493 -- crates/kernel-core/src/lib_riscv.inc.rs | head -200
  ```

- [ ] **Step 4: Check stage2 runner changes**

  ```bash
  git diff 7f5dfce..cbb8493 -- tools/dev/run_oscomp_stage2.sh | head -200
  ```

  Look for changes in: LTP profile injection, whitelist/blacklist path wiring, runtime filter logic.

- [ ] **Step 5: Apply the fix**

  Based on Steps 1-4, apply the minimal fix. This should be a control-plane fix only — no new syscall semantics.

- [ ] **Step 6: Verify locally**

  Run the minimal gate:
  ```bash
  bash tools/dev/test_run_oscomp_stage2.sh
  ```

- [ ] **Step 7: Commit**

  ```bash
  git add <changed files>
  git commit -m "oscomp: fix rv ltp score regression — restore output contract"
  ```

**QA:** After fix, a local `riscv full + musl` run must reach `whuse-oscomp-suite-done` with LTP `RUN LTP CASE` and `FAIL LTP CASE` markers present for all score-whitelisted cases.

---

### Task 2: Verify LoongArch Control Plane Stability

**Context:** LoongArch scored 10 at both 7f5dfce and cbb8493 (no regression, but also no growth). The basic-musl-la only passes test_brk (3/3) — all other basic tests score 0.

**Files:**
- Read: `assist_docs/cbb849355234d6765687d2d0ef5f3514b5f68771/LoongArch输出.md`
- Read: `crates/kernel-core/src/lib_loongarch.inc.rs`

- [ ] **Step 1: Check LoongArch output log for early termination**

  The LoongArch output is only 84 lines — extremely short. Check if it reaches `whuse-oscomp-suite-done`:
  ```bash
  grep "whuse-oscomp-suite-done\|whuse-oscomp-step-end" assist_docs/cbb849355234d6765687d2d0ef5f3514b5f68771/LoongArch输出.md
  ```

- [ ] **Step 2: Identify where LoongArch stops**

  Find the last `whuse-oscomp-step-*` marker in the log. Determine which step caused early termination.

- [ ] **Step 3: Fix if control-plane issue found**

  If early termination is due to a control-plane bug (not a semantic gap), fix it. If it's a semantic gap that requires significant kernel changes, document it and skip for now (per AGENTS.md: "keep loongarch current closed-loop path stable, no high-risk expansion").

**QA:** LoongArch `full + musl` must reach `whuse-oscomp-suite-done` with no panic/init crash.

---

## Phase 2: RV LTP Incremental Promotion Waves

### Task 3: Run Pending→Curated Promotion Wave (musl-rv)

**Context:** RV musl pending has 36 cases. Run them through the pending pipeline to promote passing cases to curated.

**Files:**
- `tools/oscomp/ltp/pending_whitelist_rv_musl.txt` (36 entries)
- `tools/oscomp/ltp/musl_rv_curated_whitelist.txt` (304 entries)
- `tools/oscomp/ltp/musl_rv_curated_blacklist.txt`

- [ ] **Step 1: Run pending wave — musl**

  ```bash
  TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=musl tools/dev/run_oscomp_stage2.sh ltp-riscv-pending
  ```

  The stage2 script will:
  - Run all pending cases
  - Classify results (pass/bad/conf)
  - Auto-promote pass cases: remove from pending whitelist, add to curated whitelist, remove from curated blacklist

- [ ] **Step 2: Run pending wave — glibc**

  ```bash
  TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv-pending
  ```

- [ ] **Step 3: Review promotion results**

  Check what changed:
  ```bash
  git diff -- tools/oscomp/ltp/pending_whitelist_rv_musl.txt
  git diff -- tools/oscomp/ltp/musl_rv_curated_whitelist.txt
  git diff -- tools/oscomp/ltp/pending_whitelist_glibc_rv.txt
  git diff -- tools/oscomp/ltp/curated_whitelist_glibc_rv.txt
  ```

- [ ] **Step 4: Commit pending→curated promotions**

  ```bash
  git add tools/oscomp/ltp/
  git commit -m "ltp-rv: promote pending→curated pass candidates (musl + glibc)"
  ```

**QA:** `bad=0 conf=0` in the curated layer after promotion.

---

### Task 4: Run Curated→Score Promotion Gate (musl-rv)

**Context:** After pending→curated promotion, run the curated stability check and score gate.

**Files:**
- `tools/oscomp/ltp/musl_rv_curated_whitelist.txt`
- `tools/oscomp/ltp/score_whitelist.txt`
- `tools/oscomp/ltp/score_blacklist.txt`

- [ ] **Step 1: Run curated stability check — musl**

  ```bash
  TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=musl tools/dev/run_oscomp_stage2.sh ltp-riscv-curated
  ```

  Must produce `bad=0 conf=0` for promotion to proceed.

- [ ] **Step 2: Run curated stability check — glibc**

  ```bash
  TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv-curated
  ```

- [ ] **Step 3: Run score gate**

  ```bash
  TIMEOUT_SECS=2400 WHUSE_LTP_PROFILE=score tools/dev/run_oscomp_stage2.sh ltp-riscv
  ```

  The stage2 script will:
  - Check curated stability (bad=0, conf=0)
  - Select up to `WHUSE_LTP_SCORE_PROMOTE_BATCH_MAX` (default 8) new pass candidates
  - Run score gate: verify all candidates pass in score profile
  - If gate passes, update score_whitelist.txt and score_blacklist.txt

- [ ] **Step 4: Review score promotion results**

  ```bash
  git diff -- tools/oscomp/ltp/score_whitelist.txt
  git diff -- tools/oscomp/ltp/score_whitelist_glibc_rv.txt
  ```

- [ ] **Step 5: Commit score promotions**

  ```bash
  git add tools/oscomp/ltp/
  git commit -m "ltp-rv: promote curated→score (batch N, <=8 cases)"
  ```

**QA:** Score gate passes with all candidates verified. No regression in existing score cases.

---

### Task 5: Repeat Promotion Waves Until Convergence

**Context:** One wave promotes at most 8 cases. Repeat Tasks 3-4 until pending is empty or no more cases pass.

- [ ] **Step 1: Repeat pending→curated→score cycle**

  Run the full cycle again:
  ```bash
  # Pending wave
  TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=musl tools/dev/run_oscomp_stage2.sh ltp-riscv-pending
  TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv-pending

  # Curated check
  TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=musl tools/dev/run_oscomp_stage2.sh ltp-riscv-curated
  TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv-curated

  # Score gate
  TIMEOUT_SECS=2400 WHUSE_LTP_PROFILE=score tools/dev/run_oscomp_stage2.sh ltp-riscv
  ```

- [ ] **Step 2: Commit each wave separately**

  Each successful wave gets its own commit with the promoted case list in the message.

**QA:** Each wave maintains `bad=0 conf=0` in curated. Score whitelist grows monotonically.

---

## Phase 3: Semantic Fixes (High-Value Blockers)

### Task 6: Fix shmctl02 (SysV Shared Memory)

**Context:** `shmctl02` is in both musl and glibc pending lists. It's a SysV IPC shared memory control test. The AGENTS.md notes "shmctl02 remains unresolved and intentionally stays out of score promotion."

**Files:**
- `crates/syscall/src/lib.rs` — `sys_shmctl` implementation
- `crates/kernel-core/src/lib_riscv.inc.rs` — blocked-restart path for shmctl
- `tools/oscomp/ltp/pending_whitelist_rv_musl.txt`
- `tools/oscomp/ltp/pending_whitelist_glibc_rv.txt`

- [ ] **Step 1: Understand shmctl02 requirements**

  shmctl02 tests various shmctl() commands: IPC_STAT, IPC_SET, IPC_RMID. Check what the test expects and what our implementation provides.

- [ ] **Step 2: Run shmctl02 in isolation**

  ```bash
  # Add shmctl02 to a minimal pending test and run
  TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=musl tools/dev/run_oscomp_stage2.sh ltp-riscv-pending
  ```

  Extract the shmctl02 output from the log to understand the failure mode.

- [ ] **Step 3: Fix the syscall implementation**

  Based on the failure mode, fix `sys_shmctl` in `crates/syscall/src/lib.rs`. Common issues:
  - IPC_STAT: returning incorrect shmid_ds structure
  - IPC_SET: not updating shm_perm fields correctly
  - IPC_RMID: not marking segment for destruction

- [ ] **Step 4: Add unit test**

  Add a syscall unit test for the shmctl path in `crates/syscall/src/lib.rs` test module.

- [ ] **Step 5: Verify and promote**

  Run the pending wave again. If shmctl02 passes, it will auto-promote to curated.

- [ ] **Step 6: Commit**

  ```bash
  git add crates/syscall/src/lib.rs tools/oscomp/ltp/
  git commit -m "syscall: fix shmctl02 — IPC_STAT/SET/RMID semantics"
  ```

**QA:** shmctl02 passes in both musl and glibc modes. Unit test covers the fixed path.

---

### Task 7: Update AGENTS.md with Current State

**Context:** After each commit batch, update the AGENTS.md runbook with the current state.

**Files:**
- `AGENTS.md`

- [ ] **Step 1: Update Section 4.1 (Current site baseline)**

  Update the score numbers, lane splits, and site-observed signals.

- [ ] **Step 2: Update Section 4.4 (Site-proven vs current local state)**

  Add new score cases promoted, note any remaining blockers.

- [ ] **Step 3: Update Section 5.2 (Next validation path)**

  Update the target markers based on current progress.

- [ ] **Step 4: Update Section 10 (Known Blocking Issues)**

  Update shmctl02 status, add any new blockers discovered.

- [ ] **Step 5: Commit**

  ```bash
  git add AGENTS.md
  git commit -m "docs(AGENTS): update current state after LTP wave N"
  ```

---

## Per-Wave Gate (Mandatory for Every Wave)

Before declaring any wave complete, run this minimum gate:

```bash
# Stage2 self-check
bash tools/dev/test_run_oscomp_stage2.sh

# Pending waves (both runtimes)
TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv-pending
TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=musl tools/dev/run_oscomp_stage2.sh ltp-riscv-pending

# Curated stability (both runtimes)
TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=glibc tools/dev/run_oscomp_stage2.sh ltp-riscv-curated
TIMEOUT_SECS=2400 WHUSE_OSCOMP_RUNTIME_FILTER=musl tools/dev/run_oscomp_stage2.sh ltp-riscv-curated

# Score gate
TIMEOUT_SECS=2400 WHUSE_LTP_PROFILE=score tools/dev/run_oscomp_stage2.sh ltp-riscv
```

**Pass criteria:**
- `whuse-oscomp-suite-done` present in all runs
- No kernel panic / init crash
- `bad=0 conf=0` in curated
- Score whitelist grows monotonically (no case removed)

---

## Execution Order

1. **Task 1** (regression diagnosis + fix) — MUST complete first
2. **Task 2** (LoongArch control plane) — parallel with Task 1 if resources allow
3. **Task 3** (pending→curated wave 1) — after Task 1
4. **Task 4** (curated→score gate 1) — after Task 3
5. **Task 5** (repeat waves) — after Task 4, iterate until convergence
6. **Task 6** (shmctl02 fix) — parallel with Tasks 3-5, or after wave convergence
7. **Task 7** (AGENTS.md update) — after each commit batch
