# LTP Score Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build LTP test infrastructure and debug existing failures to get LTP partial scores on OSCOMP.

**Architecture:** Phase 1 focus: Run LTP test to see actual pass/fail per whitelist test. Phase 2 debug deferred until results are known.

**Tech Stack:** Rust (whuse kernel), QEMU, OSCOMP testsuite, shell scripts

---

## Background Analysis

### Current State
- LTP score: 0 (all tests fail or not reached)
- Whitelist has 15 tests that should pass
- All whitelist syscalls ARE implemented (fchownat, fcntl, mmap, mremap, pwritev)
- The issue is semantic incompleteness, not missing syscalls

### Whitelist Tests (15 total)
| Test | Syscall | Count |
|------|---------|-------|
| fchownat01 | fchownat | 1 |
| fcntl02/02_64/03/03_64/04/04_64/21/21_64 | fcntl | 8 |
| mmap01, mmap11 | mmap | 2 |
| mremap01, mremap03 | mremap | 2 |
| pwritev01, pwritev01_64 | pwritev | 2 |

### Scoring Rules (from AGENTS.md)
- `WHUSE_LTP_PROFILE=score` uses whitelist/blacklist
- Pass requires: `rc==0`, `TPASS>0`, `TFAIL==0`, `TBROK==0`
- A case is "pass" when kernel emits `class=pass`

---

## Phase 1: Build LTP Test Infrastructure

### Task 1: Run LTP Score Profile on RISC-V

**Files:**
- Run: `tools/dev/run_oscomp_stage2.sh ltp-riscv`

- [ ] **Step 1: Verify environment**

Run:
```bash
export REPO_ROOT="$(pwd)"
ls -la $REPO_ROOT/kernel-rv $REPO_ROOT/target/oscomp/sdcard-rv.img
```

Expected: kernel and image exist

- [ ] **Step 2: Build images if needed**

Run:
```bash
export REPO_ROOT="$(pwd)"
export XTASK="cargo run --manifest-path $REPO_ROOT/tools/xtask/Cargo.toml --"
$XTASK oscomp-images 2>&1
```

Expected: Both sdcard-rv.img and sdcard-la.img built

- [ ] **Step 3: Run LTP score profile on RISC-V**

Run:
```bash
export REPO_ROOT="$(pwd)"
export XTASK="cargo run --manifest-path $REPO_ROOT/tools/xtask/Cargo.toml --"
TIMEOUT_SECS=2400 tools/dev/run_oscomp_stage2.sh ltp-riscv 2>&1 | tee /tmp/ltp-rv-score.log
```

This runs LTP with `WHUSE_LTP_PROFILE=score` (whitelist filtering enabled)

Expected output:
- `TPASS=N` - passing tests
- `TFAIL=N` - failing tests
- `whuse-ltp-case-result:<case>:rc=X:tpass=Y:tfail=Z:tbrok=W:class=<class>`

- [ ] **Step 4: Extract pass/fail summary**

Run:
```bash
strings /tmp/ltp-rv-score.log | grep "whuse-ltp-case-result" | head -30
strings /tmp/ltp-rv-score.log | grep "whuse-ltp-skip-case" | head -10
strings /tmp/ltp-rv-score.log | grep "\[rv-ltp\] summary"
```

Expected output:
- `whuse-ltp-case-result:<case>:rc=X:tpass=Y:tfail=Z:tbrok=W:class=<class>`
- `whuse-ltp-skip-case:<case>:filtered` (if whitelist filtered)
- `[rv-ltp] summary: TPASS=X TFAIL=Y TBROK=Z TCONF=W`

- [ ] **Step 5: Identify which whitelist tests pass vs fail**

The run generates candidate lists. Extract them:
```bash
grep "rv-ltp-pass-candidates" /tmp/ltp-rv-score.log
grep "rv-ltp-bad-candidates" /tmp/ltp-rv-score.log
```

Bad classes include: `tfail`, `tbrok`, `rc255`, `timeout`, `missing`, `nonzero`

Save whitelist pass/fail results:
```bash
echo "=== LTP Whitelist Results $(date) ===" > /tmp/whitelist-pass-fail.txt
strings /tmp/ltp-rv-score.log | grep "whuse-ltp-case-result" >> /tmp/whitelist-pass-fail.txt
```

---

### Task 2: Run LTP on LoongArch (Deferred)

**Note:** LoongArch LTP support may be limited. Focus on RISC-V first.

- [ ] **Step 1: Check if LoongArch LTP is supported**

Run:
```bash
grep -n "loongarch.*ltp\|ltp.*loongarch" tools/dev/run_oscomp_stage2.sh
```

If not supported, defer LoongArch LTP to future work.

---

## Next Steps (After Getting First LTP Points)

1. Get first LTP score points (> 0)
2. Expand whitelist based on what's actually passing
3. Debug remaining failing whitelist tests
4. Consider adding missing syscalls (msg*/sem*) for broader coverage
