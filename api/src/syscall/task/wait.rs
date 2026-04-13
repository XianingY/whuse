use alloc::vec::Vec;
use core::{future::poll_fn, task::Poll};

use axerrno::{LinuxError, LinuxResult};
use axhal::{context::TrapFrame, time::TimeValue};
use axtask::{current, future::try_block_on};
use bitflags::bitflags;
use linux_raw_sys::general::{
    __WALL, __WCLONE, __WNOTHREAD, CLD_EXITED, P_ALL, P_PGID, P_PID, WCONTINUED, WEXITED, WNOHANG,
    WNOWAIT, WUNTRACED, siginfo,
};
use starry_core::task::AsThread;
use starry_process::{Pid, Process};
use starry_signal::{SignalInfo, Signo};
use starry_vm::{VmMutPtr, VmPtr};

use crate::signal::check_signals;

bitflags! {
    #[derive(Debug, Clone, Copy)]
    struct WaitOptions: u32 {
        /// Do not block when there are no processes wishing to report status.
        const WNOHANG = WNOHANG;
        /// Report the status of selected processes which are stopped due to a
        /// `SIGTTIN`, `SIGTTOU`, `SIGTSTP`, or `SIGSTOP` signal.
        const WUNTRACED = WUNTRACED;
        /// Report the status of selected processes which have terminated.
        const WEXITED = WEXITED;
        /// Report the status of selected processes that have continued from a
        /// job control stop by receiving a `SIGCONT` signal.
        const WCONTINUED = WCONTINUED;
        /// Don't reap, just poll status.
        const WNOWAIT = WNOWAIT;

        /// Don't wait on children of other threads in this group
        const WNOTHREAD = __WNOTHREAD;
        /// Wait on all children, regardless of type
        const WALL = __WALL;
        /// Wait for "clone" children only.
        const WCLONE = __WCLONE;
    }
}

#[derive(Debug, Clone, Copy)]
enum WaitPid {
    /// Wait for any child process
    Any,
    /// Wait for the child whose process ID is equal to the value.
    Pid(Pid),
    /// Wait for any child process whose process group ID is equal to the value.
    Pgid(Pid),
}

impl WaitPid {
    fn apply(&self, child: &Process) -> bool {
        match self {
            WaitPid::Any => true,
            WaitPid::Pid(pid) => child.pid() == *pid,
            WaitPid::Pgid(pgid) => child.group().pgid() == *pgid,
        }
    }
}

fn filtered_children(proc: &Process, pid: WaitPid) -> Vec<alloc::sync::Arc<Process>> {
    proc.children()
        .into_iter()
        .filter(|child| pid.apply(child))
        .collect::<Vec<_>>()
}

fn wait_options(options: u32) -> LinuxResult<WaitOptions> {
    WaitOptions::from_bits(options).ok_or(LinuxError::EINVAL)
}

fn wait_pid_selector(pid: i32, pgid: Pid) -> LinuxResult<WaitPid> {
    Ok(if pid == -1 {
        WaitPid::Any
    } else if pid == 0 {
        WaitPid::Pgid(pgid)
    } else if pid > 0 {
        WaitPid::Pid(pid as _)
    } else if pid == i32::MIN {
        return Err(LinuxError::ESRCH);
    } else {
        WaitPid::Pgid((-pid) as _)
    })
}

fn check_wait_children(
    children: &[alloc::sync::Arc<Process>],
    exit_code: *mut i32,
    options: WaitOptions,
) -> LinuxResult<isize> {
    if let Some(child) = children.iter().find(|child| child.waitable()) {
        if let Some(exit_code) = exit_code.nullable() {
            exit_code.vm_write(child.exit_status())?;
        }
        if !options.contains(WaitOptions::WNOWAIT) {
            let (utime_ns, stime_ns) = child.exit_cpu_times();
            current().as_thread().proc_data.add_child_cpu_times(
                TimeValue::from_nanos(utime_ns),
                TimeValue::from_nanos(stime_ns),
            );
            child.consume_wait();
            child.free();
        }
        Ok(child.pid() as _)
    } else if options.contains(WaitOptions::WNOHANG) {
        Ok(0)
    } else {
        Err(LinuxError::EAGAIN)
    }
}

fn write_waitid_info(info: *mut siginfo, child: &Process) -> LinuxResult<()> {
    if let Some(info) = info.nullable() {
        let mut data = SignalInfo::new_user(Signo::SIGCHLD, CLD_EXITED as i32, child.pid());
        data.0
            .__bindgen_anon_1
            .__bindgen_anon_1
            ._sifields
            ._sigchld
            ._status = child.exit_status() >> 8;
        info.vm_write(data.0)?;
    }
    Ok(())
}

fn check_waitid_children(
    children: &[alloc::sync::Arc<Process>],
    info: *mut siginfo,
    options: WaitOptions,
) -> LinuxResult<isize> {
    if let Some(child) = children.iter().find(|child| child.waitable()) {
        write_waitid_info(info, child)?;
        if !options.contains(WaitOptions::WNOWAIT) {
            let (utime_ns, stime_ns) = child.exit_cpu_times();
            current().as_thread().proc_data.add_child_cpu_times(
                TimeValue::from_nanos(utime_ns),
                TimeValue::from_nanos(stime_ns),
            );
            child.consume_wait();
            child.free();
        }
        Ok(0)
    } else if options.contains(WaitOptions::WNOHANG) {
        if let Some(info) = info.nullable() {
            info.vm_write(unsafe { core::mem::zeroed() })?;
        }
        Ok(0)
    } else {
        Err(LinuxError::EAGAIN)
    }
}

pub fn sys_waitpid(
    tf: &mut TrapFrame,
    pid: i32,
    exit_code: *mut i32,
    options: u32,
) -> LinuxResult<isize> {
    let options = wait_options(options)?;
    info!("sys_waitpid <= pid: {:?}, options: {:?}", pid, options);

    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let proc = &proc_data.proc;

    let pid = wait_pid_selector(pid, proc.group().pgid())?;

    // FIXME: add back support for WALL & WCLONE, since ProcessData may drop before
    // Process now.
    let children = filtered_children(proc, pid);
    if children.is_empty() {
        return Err(LinuxError::ECHILD);
    }

    let result = try_block_on(poll_fn(|cx| {
        match check_wait_children(&children, exit_code, options) {
            Ok(pid) => Poll::Ready(Ok(pid)),
            Err(LinuxError::EAGAIN) => {
                proc_data.child_exit_event.register(cx.waker());
                match check_wait_children(&children, exit_code, options) {
                    Ok(pid) => Poll::Ready(Ok(pid)),
                    Err(LinuxError::EAGAIN) => Poll::Pending,
                    other => Poll::Ready(other),
                }
            }
            other => Poll::Ready(other),
        }
    }));
    match result {
        Ok(Some(result)) => Ok(result),
        Ok(None) => {
            // RESTART
            tf.set_ip(tf.ip() - 4);
            while check_signals(curr.as_thread(), tf, None) {}
            Ok(0)
        }
        Err(err) => Err(err),
    }
}

pub fn sys_waitid(
    tf: &mut TrapFrame,
    idtype: u32,
    id: u32,
    info: *mut siginfo,
    options: u32,
) -> LinuxResult<isize> {
    let options = wait_options(options)?;
    if !options.contains(WaitOptions::WEXITED) {
        return Err(LinuxError::EINVAL);
    }

    let curr = current();
    let proc_data = &curr.as_thread().proc_data;
    let proc = &proc_data.proc;
    let selector = match idtype {
        P_ALL => WaitPid::Any,
        P_PID => WaitPid::Pid(id),
        P_PGID => WaitPid::Pgid(if id == 0 { proc.group().pgid() } else { id }),
        _ => return Err(LinuxError::EINVAL),
    };
    let children = filtered_children(proc, selector);
    if children.is_empty() {
        return Err(LinuxError::ECHILD);
    }

    let result = try_block_on(poll_fn(|cx| {
        match check_waitid_children(&children, info, options) {
            Ok(status) => Poll::Ready(Ok(status)),
            Err(LinuxError::EAGAIN) => {
                proc_data.child_exit_event.register(cx.waker());
                match check_waitid_children(&children, info, options) {
                    Ok(status) => Poll::Ready(Ok(status)),
                    Err(LinuxError::EAGAIN) => Poll::Pending,
                    other => Poll::Ready(other),
                }
            }
            other => Poll::Ready(other),
        }
    }));
    match result {
        Ok(Some(result)) => Ok(result),
        Ok(None) => {
            tf.set_ip(tf.ip() - 4);
            while check_signals(curr.as_thread(), tf, None) {}
            Ok(0)
        }
        Err(err) => Err(err),
    }
}
