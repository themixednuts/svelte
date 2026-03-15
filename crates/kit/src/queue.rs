use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::sync::{Mutex, Notify, Semaphore, oneshot};

type BoxFuture<T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'static>>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Cannot add tasks to a queue that has ended")]
pub struct QueueClosedError;

#[derive(Clone)]
pub struct AsyncQueue<T, E> {
    semaphore: Arc<Semaphore>,
    state: Arc<Mutex<QueueState<E>>>,
    notify: Arc<Notify>,
    _marker: std::marker::PhantomData<fn() -> T>,
}

#[derive(Debug)]
struct QueueState<E> {
    pending: usize,
    closed: bool,
    first_error: Option<E>,
}

pub struct TaskHandle<T, E> {
    receiver: oneshot::Receiver<Result<T, E>>,
}

pub fn queue<T, E>(concurrency: usize) -> AsyncQueue<T, E> {
    AsyncQueue {
        semaphore: Arc::new(Semaphore::new(concurrency.max(1))),
        state: Arc::new(Mutex::new(QueueState {
            pending: 0,
            closed: false,
            first_error: None,
        })),
        notify: Arc::new(Notify::new()),
        _marker: std::marker::PhantomData,
    }
}

impl<T, E> AsyncQueue<T, E>
where
    T: Send + 'static,
    E: Clone + Send + 'static,
{
    pub async fn add<F, Fut>(&self, task: F) -> Result<TaskHandle<T, E>, QueueClosedError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        let mut state = self.state.lock().await;
        if state.closed {
            return Err(QueueClosedError);
        }
        state.pending += 1;
        drop(state);

        let semaphore = Arc::clone(&self.semaphore);
        let state = Arc::clone(&self.state);
        let notify = Arc::clone(&self.notify);
        let (sender, receiver) = oneshot::channel();
        let task: BoxFuture<T, E> = Box::pin(task());

        tokio::spawn(async move {
            let permit = semaphore
                .acquire_owned()
                .await
                .expect("queue semaphore should not be closed");
            let result = task.await;
            drop(permit);

            let mut queue_state = state.lock().await;
            if let Err(error) = &result
                && queue_state.first_error.is_none()
            {
                queue_state.first_error = Some(error.clone());
            }
            queue_state.pending = queue_state.pending.saturating_sub(1);
            if queue_state.pending == 0 {
                queue_state.closed = true;
            }
            drop(queue_state);

            let _ = sender.send(result);
            notify.notify_waiters();
        });

        Ok(TaskHandle { receiver })
    }

    pub async fn done(&self) -> Result<(), E> {
        {
            let mut state = self.state.lock().await;
            if state.pending == 0 {
                state.closed = true;
                return Ok(());
            }
        }

        loop {
            let notified = self.notify.notified();
            {
                let mut state = self.state.lock().await;
                if let Some(error) = &state.first_error {
                    return Err(error.clone());
                }
                if state.pending == 0 {
                    state.closed = true;
                    return Ok(());
                }
            }
            notified.await;
        }
    }
}

impl<T, E> Future for TaskHandle<T, E> {
    type Output = Result<T, E>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.receiver).poll(cx) {
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(_)) => panic!("queue task sender dropped before completing"),
            Poll::Pending => Poll::Pending,
        }
    }
}
