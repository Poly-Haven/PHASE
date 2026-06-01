use std::collections::VecDeque;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use crate::validation::{all_checks, send_finished, Finding, Msg, Request, ValidationContext};

pub const WORKER_COUNT: usize = 8;
const WEIGHT_CAPACITY: usize = 8;

struct Task {
    request_index: usize,
    check_index: usize,
}

struct TaskResult {
    request_index: usize,
    check_index: usize,
    findings: Vec<Finding>,
}

pub fn spawn(requests: Vec<Request>, tx: Sender<Msg>) {
    thread::spawn(move || run(requests, tx));
}

fn run(requests: Vec<Request>, tx: Sender<Msg>) {
    if requests.is_empty() {
        send_finished(&tx);
        return;
    }

    let checks = all_checks().to_vec();
    let task_count = requests.len() * checks.len();
    if task_count == 0 {
        for request in requests {
            if tx
                .send(Msg::RowValidated {
                    key: request.key,
                    findings: Vec::new(),
                })
                .is_err()
            {
                return;
            }
        }
        send_finished(&tx);
        return;
    }

    let tasks = (0..requests.len())
        .flat_map(|request_index| {
            (0..checks.len()).map(move |check_index| Task {
                request_index,
                check_index,
            })
        })
        .collect::<VecDeque<_>>();

    let requests = Arc::new(requests);
    let checks = Arc::new(checks);
    let queue = Arc::new(Mutex::new(tasks));
    let limiter = Arc::new(WeightedLimiter::new(WEIGHT_CAPACITY));
    let (result_tx, result_rx) = channel();

    let mut workers = Vec::with_capacity(WORKER_COUNT);
    for _ in 0..WORKER_COUNT {
        let requests = Arc::clone(&requests);
        let checks = Arc::clone(&checks);
        let queue = Arc::clone(&queue);
        let limiter = Arc::clone(&limiter);
        let result_tx = result_tx.clone();
        workers.push(thread::spawn(move || loop {
            let Some(task) = queue.lock().unwrap().pop_front() else {
                break;
            };
            let check = checks[task.check_index];
            let _permit = limiter.acquire(check.weight());
            let ctx = ValidationContext::from(&requests[task.request_index]);
            let findings = check.run(&ctx);
            if result_tx
                .send(TaskResult {
                    request_index: task.request_index,
                    check_index: task.check_index,
                    findings,
                })
                .is_err()
            {
                break;
            }
        }));
    }
    drop(result_tx);

    let mut findings_by_request = vec![vec![Vec::<Finding>::new(); checks.len()]; requests.len()];
    let mut remaining_by_request = vec![checks.len(); requests.len()];
    for _ in 0..task_count {
        let Ok(result) = result_rx.recv() else {
            break;
        };
        findings_by_request[result.request_index][result.check_index] = result.findings;
        remaining_by_request[result.request_index] -= 1;
        if remaining_by_request[result.request_index] == 0 {
            let findings = findings_by_request[result.request_index]
                .iter()
                .flat_map(|findings| findings.iter().cloned())
                .collect();
            if tx
                .send(Msg::RowValidated {
                    key: requests[result.request_index].key.clone(),
                    findings,
                })
                .is_err()
            {
                break;
            }
        }
    }

    for worker in workers {
        let _ = worker.join();
    }
    send_finished(&tx);
}

pub(crate) struct WeightedLimiter {
    state: Mutex<WeightedLimiterState>,
    available: Condvar,
}

struct WeightedLimiterState {
    capacity: usize,
    in_use: usize,
}

impl WeightedLimiter {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(WeightedLimiterState {
                capacity,
                in_use: 0,
            }),
            available: Condvar::new(),
        }
    }

    pub(crate) fn acquire(self: &Arc<Self>, weight: usize) -> WeightedPermit {
        let weight = weight.max(1);
        let mut state = self.state.lock().unwrap();
        while state.in_use + weight > state.capacity {
            state = self.available.wait(state).unwrap();
        }
        state.in_use += weight;
        WeightedPermit {
            limiter: Arc::clone(self),
            weight,
        }
    }

    #[cfg(test)]
    pub(crate) fn try_acquire_for_test(self: &Arc<Self>, weight: usize) -> Option<WeightedPermit> {
        let weight = weight.max(1);
        let mut state = self.state.lock().unwrap();
        if state.in_use + weight > state.capacity {
            return None;
        }
        state.in_use += weight;
        Some(WeightedPermit {
            limiter: Arc::clone(self),
            weight,
        })
    }

    fn release(&self, weight: usize) {
        let mut state = self.state.lock().unwrap();
        state.in_use = state.in_use.saturating_sub(weight);
        self.available.notify_all();
    }
}

pub(crate) struct WeightedPermit {
    limiter: Arc<WeightedLimiter>,
    weight: usize,
}

impl Drop for WeightedPermit {
    fn drop(&mut self) {
        self.limiter.release(self.weight);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn validation_uses_eight_workers() {
        assert_eq!(WORKER_COUNT, 8);
    }

    #[test]
    fn weighted_limiter_blocks_when_capacity_is_exhausted() {
        let limiter = Arc::new(WeightedLimiter::new(8));
        let first = limiter.acquire(6);
        let second = limiter.try_acquire_for_test(2);

        assert!(second.is_some());
        assert!(limiter.try_acquire_for_test(1).is_none());

        drop(second);
        drop(first);
        assert!(limiter.try_acquire_for_test(1).is_some());
    }
}
