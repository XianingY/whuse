#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Exited,
}

#[derive(Clone, Debug)]
pub struct Task {
    pub id: usize,
    pub process_id: usize,
    pub name: String,
    pub state: TaskState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitToken {
    pub queue_id: usize,
    pub task_id: usize,
}

#[derive(Clone, Debug, Default)]
pub struct WaitQueue {
    pub id: usize,
    waiters: VecDeque<usize>,
}

impl WaitQueue {
    pub fn new(id: usize) -> Self {
        Self {
            id,
            waiters: VecDeque::new(),
        }
    }

    pub fn register(&mut self, task_id: usize) -> WaitToken {
        if !self.waiters.iter().any(|waiter| *waiter == task_id) {
            self.waiters.push_back(task_id);
        }
        WaitToken {
            queue_id: self.id,
            task_id,
        }
    }

    pub fn wake_one(&mut self) -> Option<usize> {
        self.waiters.pop_front()
    }

    pub fn wake_all(&mut self) -> Vec<usize> {
        self.waiters.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.waiters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.waiters.is_empty()
    }
}

#[derive(Debug)]
pub struct Scheduler {
    next_id: usize,
    ready: VecDeque<Task>,
    current: Option<Task>,
    blocked: BTreeMap<usize, Task>,
    next_wait_queue: usize,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            ready: VecDeque::new(),
            current: None,
            blocked: BTreeMap::new(),
            next_wait_queue: 1,
        }
    }

    pub fn create_wait_queue(&mut self) -> WaitQueue {
        let id = self.next_wait_queue;
        self.next_wait_queue += 1;
        WaitQueue::new(id)
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

    pub fn ensure_current(&mut self) -> Option<usize> {
        if let Some(current) = &self.current {
            Some(current.id)
        } else {
            self.schedule_next()
        }
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

    pub fn current_process_id(&self) -> Option<usize> {
        self.current.as_ref().map(|task| task.process_id)
    }

    pub fn block_current_on(&mut self, queue: &mut WaitQueue) -> Option<WaitToken> {
        let mut current = self.current.take()?;
        current.state = TaskState::Blocked;
        let token = queue.register(current.id);
        self.blocked.insert(current.id, current);
        let _ = self.schedule_next();
        Some(token)
    }

    pub fn wake_task(&mut self, task_id: usize) -> bool {
        let Some(mut task) = self.blocked.remove(&task_id) else {
            return false;
        };
        task.state = TaskState::Ready;
        self.ready.push_back(task);
        true
    }

    pub fn wake_one(&mut self, queue: &mut WaitQueue) -> Option<usize> {
        let task_id = queue.wake_one()?;
        let _ = self.wake_task(task_id);
        Some(task_id)
    }

    pub fn wake_all(&mut self, queue: &mut WaitQueue) -> usize {
        let task_ids = queue.wake_all();
        let mut woke = 0;
        for task_id in task_ids {
            if self.wake_task(task_id) {
                woke += 1;
            }
        }
        woke
    }

    pub fn blocked_count(&self) -> usize {
        self.blocked.len()
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

    #[test]
    fn wait_queue_round_trip() {
        let mut scheduler = Scheduler::new();
        let task_a = scheduler.spawn("a", 1);
        let task_b = scheduler.spawn("b", 2);
        let mut queue = scheduler.create_wait_queue();

        assert_eq!(scheduler.start(), Some(task_a));
        let token = scheduler.block_current_on(&mut queue).unwrap();
        assert_eq!(token.task_id, task_a);
        assert_eq!(scheduler.current().unwrap().id, task_b);
        assert_eq!(scheduler.blocked_count(), 1);

        assert_eq!(scheduler.wake_one(&mut queue), Some(task_a));
        assert_eq!(scheduler.blocked_count(), 0);
        assert_eq!(scheduler.yield_now(), Some(task_a));
    }
}
