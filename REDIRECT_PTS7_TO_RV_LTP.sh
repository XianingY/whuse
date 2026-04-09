#!/bin/bash
# ============================================================
# CLAUDE CODE PROCESS REDIRECTION SCRIPT
# ============================================================
# Run this in pts/7's Claude Code terminal to redirect it
# from biframe evaluation to RISC-V LTP pending case work
# ============================================================

echo "=========================================="
echo "PARALLELIZATION: RISC-V LTP from LoongArch Claude"
echo "=========================================="
echo ""
echo "CURRENT STATE:"
echo "  - pts/5 (pid 26030): running 'make oscomp-loongarch'"
echo "  - pts/7 (pid 41041): running 'eval-vs-biframe.sh riscv O1'"
echo ""
echo "GOAL: Redirect pts/7 from biframe to RISC-V LTP timerfd implementation"
echo ""
echo "=========================================="
echo "COPY THE FOLLOWING BLOCK INTO pts/7's CLAUDE CODE TERMINAL:"
echo "=========================================="

cat <<'EOF'

ultrawork [ANALYSIS COMPLETE - EXECUTE NOW]

在当前pts/7进程执行以下操作来并行推进RISC-V LTP:

1. 先查看当前工作状态:
   pwd && git branch

2. 切换到rv-ltp-timerfd worktree:
   cd /home/wslootie/github/whuse/.worktrees/rv-ltp-timerfd

3. 用ultrawork模式实现timerfd_create syscall:

TASK: 实现缺失的timerfd syscalls以解除14个pending LTP case

1. 在 crates/syscall/src/lib.rs 添加:
   pub const SYS_TIMERFD_CREATE: usize = 271;
   pub const SYS_TIMERFD_SETTIME: usize = 272;
   pub const SYS_TIMERFD_GETTIME: usize = 273;
   pub const SYS_TIMER_CREATE: usize = 223;
   pub const SYS_TIMER_DELETE: usize = 224;
   pub const SYS_TIMER_SETTIME: usize = 226;
   pub const SYS_TIMER_GETTIME: usize = 225;
   pub const SYS_TIMER_GETOVERRUN: usize = 227;

2. 在 crates/syscall/src/time_domain.rs dispatch() 添加:
   SYS_TIMERFD_CREATE => ctx.dispatcher.sys_timerfd_create(args, ctx.procs),
   SYS_TIMERFD_SETTIME => ctx.dispatcher.sys_timerfd_settime(args, ctx.procs),
   SYS_TIMERFD_GETTIME => ctx.dispatcher.sys_timerfd_gettime(args, ctx.procs),
   SYS_TIMER_CREATE => ctx.dispatcher.sys_timer_create(args, ctx.procs),
   SYS_TIMER_DELETE => ctx.dispatcher.sys_timer_delete(args, ctx.procs),
   SYS_TIMER_SETTIME => ctx.dispatcher.sys_timer_settime(args, ctx.procs),
   SYS_TIMER_GETTIME => ctx.dispatcher.sys_timer_gettime(args, ctx.procs),
   SYS_TIMER_GETOVERRUN => ctx.dispatcher.sys_timer_getoverrun(args, ctx.procs),

3. 实现每个函数,参考 clock_gettime/clock_nanosleep 模式

4. 构建验证: cargo build --release 2>&1 | tail -20

5. 测试: WHUSE_LTP_PROFILE=pending WHUSE_OSCOMP_RUNTIME_FILTER=musl tools/dev/run_oscomp_stage2.sh ltp-riscv 2>&1 | tail -50

注意: timerfd是一个文件描述符,到期时产生可读事件,类似pipe的语义

EOF

echo ""
echo "=========================================="
echo "OR ALTERNATIVELY - Redirect to semantic fixes:"
echo "=========================================="
cat <<'EOF'

# 替代方案: 修复fcntl17_64等语义问题cases

cd /home/wslootie/github/whuse/.worktrees/rv-ltp-semantic

TASK: 修复以下pending cases的语义问题:
- fcntl17_64: fcntl F_GETLK64/F_SETLK64 64位锁结构
- ftruncate04_64: 大文件truncate (>2GB)
- vfork01/02: CLONE_VFORK 语义
- openat04: openat O_* flags
- shmctl02: SysV IPC shmctl命令

先分析一个case失败原因:
WHUSE_LTP_PROFILE=pending WHUSE_OSCOMP_RUNTIME_FILTER=musl \
  WHUSE_OSCOMP_CASE_FILTER=fcntl17_64 \
  tools/dev/run_oscomp_stage2.sh ltp-riscv 2>&1 | tail -100

然后根据输出分析修复

EOF

echo ""
echo "=========================================="
echo "同时,在pts/5的LoongArch进程继续运行"
echo "不需要停止,让它继续完成make oscomp-loongarch"
echo "=========================================="
