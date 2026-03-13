#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Ready,
    Running,
    Exited,
}

#[derive(Clone, Debug)]
pub struct Task {
    pub id: usize,
    pub process_id: usize,
    pub name: String,
    pub state: TaskState,
}

#[derive(Debug)]
pub struct Scheduler {
    next_id: usize,
    ready: VecDeque<Task>,
    current: Option<Task>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            ready: VecDeque::new(),
            current: None,
        }
    }

    pub fn spawn(&mut self, name: &str, process_id: usize) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.ready.push_back(Task {
            id,
            process_id,
            name: name.to_string(),
            state: TaskState::Ready,
        });
        id
    }

    pub fn start(&mut self) -> Option<usize> {
        self.schedule_next()
    }

    pub fn yield_now(&mut self) -> Option<usize> {
        if let Some(mut current) = self.current.take() {
            if current.state != TaskState::Exited {
                current.state = TaskState::Ready;
                self.ready.push_back(current);
            }
        }
        self.schedule_next()
    }

    pub fn exit_current(&mut self) {
        if let Some(mut current) = self.current.take() {
            current.state = TaskState::Exited;
        }
    }

    pub fn current(&self) -> Option<&Task> {
        self.current.as_ref()
    }

    fn schedule_next(&mut self) -> Option<usize> {
        let mut task = self.ready.pop_front()?;
        task.state = TaskState::Running;
        let id = task.id;
        self.current = Some(task);
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::Scheduler;

    #[test]
    fn round_robin_scheduler() {
        let mut scheduler = Scheduler::new();
        let a = scheduler.spawn("a", 1);
        let b = scheduler.spawn("b", 2);
        assert_eq!(scheduler.start(), Some(a));
        assert_eq!(scheduler.yield_now(), Some(b));
    }
}

