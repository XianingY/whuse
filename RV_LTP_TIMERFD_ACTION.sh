#!/bin/bash
# ============================================================
# RISC-V LTP Timerfd Implementation - Action Script
# ============================================================
# Usage:
#   Option A: Copy the 'SEND TO CLAUDE CODE' block below into pts/7 Claude Code
#   Option B: Run this script directly in a new terminal
# ============================================================

set -e
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

echo "=== RISC-V LTP Timerfd Implementation ==="
echo "Repo: $REPO_ROOT"
echo "Worktree: $(pwd)"

# Step 1: Verify we have the right setup
echo ""
echo "[Step 1] Verifying setup..."
if grep -q "timerfd_create" crates/syscall/src/lib.rs 2>/dev/null; then
	echo "  timerfd_create ALREADY IMPLEMENTED"
else
	echo "  timerfd_create MISSING - needs implementation"
fi

# Step 2: Show missing syscalls
echo ""
echo "[Step 2] Missing timerfd/timer syscalls:"
grep -E "^pub const SYS_TIMER" crates/syscall/src/lib.rs | grep -iE "timerfd|timer" || echo "  None found (expected - they need implementation)"

# Step 3: Show pending cases
echo ""
echo "[Step 3] Pending LTP cases blocked by missing timerfd:"
echo "  timerfd_create01, timerfd_settime01, timerfd_settime02, timerfd_gettime01"
echo "  timer_settime01, timer_settime02, timer_settime03"
echo "  timer_gettime01, timer_delete01, timer_delete02, timer_getoverrun01"

# Step 4: Show next steps
echo ""
echo "[Step 4] Next Steps for Claude Code:"
echo "  1. In crates/syscall/src/lib.rs, add:"
echo "     pub const SYS_TIMERFD_CREATE: usize = 271;"
echo "     pub const SYS_TIMERFD_SETTIME: usize = 272;"
echo "     pub const SYS_TIMERFD_GETTIME: usize = 273;"
echo "     pub const SYS_TIMER_CREATE: usize = 223;"
echo "     pub const SYS_TIMER_DELETE: usize = 224;"
echo "     pub const SYS_TIMER_GETTIME: usize = 225;"
echo "     pub const SYS_TIMER_SETTIME: usize = 226;"
echo "     pub const SYS_TIMER_GETOVERRUN: usize = 227;"
echo "  4. Run: cargo build --release"
echo "  5. Run LTP test: WHUSE_LTP_PROFILE=pending tools/dev/run_oscomp_stage2.sh ltp-riscv"

echo ""
echo "=== VERIFICATION COMMANDS ==="
echo "Build: cargo build --release 2>&1 | tail -5"
echo "Check syscalls: grep -E 'SYS_TIMERFD|SYS_TIMER' crates/syscall/src/lib.rs"
echo "Run pending LTP: WHUSE_LTP_PROFILE=pending WHUSE_OSCOMP_RUNTIME_FILTER=musl tools/dev/run_oscomp_stage2.sh ltp-riscv 2>&1 | tail -50"

# ============================================================
# COPY BELOW THIS LINE AND PASTE INTO CLAUDE CODE (pts/7)
# ============================================================
cat <<'CLAUDE_INSTRUCTION'

在当前终端执行以下ultrawork任务:

用ultrawork模式实现RISC-V缺失的timerfd syscalls:

TASK: 在 crates/syscall/src/ 实现以下timerfd相关syscalls:
1. timerfd_create (syscall#271) - 创建timerfd文件描述符
2. timerfd_settime (syscall#272) - 设置timer到期时间
3. timerfd_gettime (syscall#273) - 获取timerfd当前值
4. timer_create (syscall#223), timer_delete (syscall#224), timer_gettime (syscall#225), timer_settime (syscall#226), timer_getoverrun (syscall#227)

实现步骤:
1. 在 crates/syscall/src/lib.rs 添加:
   pub const SYS_TIMERFD_CREATE: usize = 271;
   pub const SYS_TIMERFD_SETTIME: usize = 272;
   pub const SYS_TIMERFD_GETTIME: usize = 273;
   pub const SYS_TIMER_CREATE: usize = 223;
   pub const SYS_TIMER_DELETE: usize = 224;
   pub const SYS_TIMER_GETTIME: usize = 225;
   pub const SYS_TIMER_SETTIME: usize = 226;
   pub const SYS_TIMER_GETOVERRUN: usize = 227;

2. 在 crates/syscall/src/time_domain.rs 的 dispatch 函数添加case:
   SYS_TIMERFD_CREATE => ctx.dispatcher.sys_timerfd_create(args, ctx.procs),
   SYS_TIMERFD_SETTIME => ctx.dispatcher.sys_timerfd_settime(args, ctx.procs),
   SYS_TIMERFD_GETTIME => ctx.dispatcher.sys_timerfd_gettime(args, ctx.procs),
   ... 等等

3. 实现每个函数,参考现有的 clock_gettime/clock_nanosleep 实现

4. 验证构建: cargo build --release 2>&1 | tail -20

5. 如果构建成功,运行LTP测试:
   WHUSE_LTP_PROFILE=pending WHUSE_OSCOMP_RUNTIME_FILTER=musl \
     tools/dev/run_oscomp_stage2.sh ltp-riscv 2>&1 | tail -100

参考:
- Linux man: timerfd_create(2), timerfd_settime(2), timerfd_gettime(2)
- Linux man: timer_create(2), timer_settime(2), timer_gettime(2)
- 现有实现: crates/syscall/src/time_domain.rs
CLAUDE_INSTRUCTION
echo ""
echo "=== COPY ABOVE THIS LINE AND PASTE INTO CLAUDE CODE ==="
