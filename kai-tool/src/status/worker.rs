//! One in-flight operation per worker. Cancellation also bounds helper lifetimes.
use anyhow::{Context, Result, ensure};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
};
use std::thread::{self, JoinHandle};

#[derive(Clone, Default)]
pub(super) struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub(super) fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub(super) fn check(&self) -> Result<()> {
        ensure!(!self.cancelled(), "Operation cancelled");
        Ok(())
    }

    pub(super) fn cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub(super) struct Worker<I, O> {
    requests: Option<Sender<(I, Cancellation)>>,
    responses: Receiver<Result<O>>,
    thread: Option<JoinHandle<()>>,
    current: Option<Cancellation>,
}

impl<I: Send + 'static, O: Send + 'static> Worker<I, O> {
    pub(super) fn new(
        mut operation: impl FnMut(I, &Cancellation) -> Result<O> + Send + 'static,
    ) -> Self {
        let (requests, receive) = mpsc::channel::<(I, Cancellation)>();
        let (send, responses) = mpsc::channel();
        let thread = thread::spawn(move || {
            while let Ok((input, cancel)) = receive.recv() {
                let result = cancel.check().and_then(|()| operation(input, &cancel));
                if send.send(result).is_err() {
                    break;
                }
            }
        });
        Self {
            requests: Some(requests),
            responses,
            thread: Some(thread),
            current: None,
        }
    }

    pub(super) fn busy(&self) -> bool {
        self.current.is_some()
    }

    pub(super) fn start(&mut self, input: I) -> Result<()> {
        // Ignore repeated Enter/refreshes until the previous operation has been collected.
        if self.busy() {
            return Ok(());
        }
        let cancel = Cancellation::default();
        self.requests
            .as_ref()
            .unwrap()
            .send((input, cancel.clone()))
            .map_err(|_| anyhow::anyhow!("Worker disconnected"))
            .context("Status worker stopped unexpectedly")?;
        self.current = Some(cancel);
        Ok(())
    }

    pub(super) fn poll(&mut self) -> Option<Result<O>> {
        let result = match self.responses.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) if self.busy() => {
                Err(anyhow::anyhow!("Status worker stopped unexpectedly"))
            }
            Err(_) => return None,
        };
        let cancelled = self.current.take().is_some_and(|cancel| cancel.cancelled());
        if cancelled { None } else { Some(result) }
    }

    pub(super) fn cancel(&self) {
        if let Some(cancel) = &self.current {
            cancel.cancel();
        }
    }
}

impl<I, O> Drop for Worker<I, O> {
    fn drop(&mut self) {
        if let Some(cancel) = &self.current {
            cancel.cancel();
        }
        self.requests.take();
        // Reap cancelled desktop helpers before the dashboard process exits.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_busy_worker_does_not_queue_requests_and_cancellation_discards_late_results() {
        let (started, wait_started) = mpsc::channel();
        let mut worker = Worker::new(move |value: u32, cancel: &Cancellation| {
            started.send(value).unwrap();
            while !cancel.cancelled() {
                thread::sleep(Duration::from_millis(1));
            }
            Ok(value)
        });
        worker.start(1).unwrap();
        assert_eq!(
            wait_started.recv_timeout(Duration::from_secs(2)).unwrap(),
            1
        );
        worker.start(2).unwrap();
        worker.cancel();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while worker.busy() {
            assert!(worker.poll().is_none());
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(wait_started.try_recv().is_err());
        worker.start(3).unwrap();
        assert_eq!(
            wait_started.recv_timeout(Duration::from_secs(2)).unwrap(),
            3
        );
        drop(worker); // Cancels the running operation before joining.
    }
}
