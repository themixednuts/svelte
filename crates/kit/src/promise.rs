use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

#[derive(Debug, PartialEq, Eq)]
pub enum PromiseState<T, E> {
    Resolved(T),
    Rejected(E),
    Cancelled,
}

#[derive(Clone)]
pub struct PromiseWithResolvers<T, E> {
    sender: Arc<Mutex<Option<oneshot::Sender<PromiseState<T, E>>>>>,
    receiver: Arc<Mutex<Option<oneshot::Receiver<PromiseState<T, E>>>>>,
}

impl<T, E> PromiseWithResolvers<T, E> {
    pub fn resolve(&self, value: T) -> bool {
        self.send(PromiseState::Resolved(value))
    }

    pub fn reject(&self, error: E) -> bool {
        self.send(PromiseState::Rejected(error))
    }

    pub async fn promise(&self) -> PromiseState<T, E> {
        let receiver = self
            .receiver
            .lock()
            .expect("receiver lock should be available")
            .take();

        let Some(receiver) = receiver else {
            return PromiseState::Cancelled;
        };

        receiver.await.unwrap_or(PromiseState::Cancelled)
    }

    fn send(&self, value: PromiseState<T, E>) -> bool {
        self.sender
            .lock()
            .expect("sender lock should be available")
            .take()
            .map(|sender| sender.send(value).is_ok())
            .unwrap_or(false)
    }
}

pub fn with_resolvers<T, E>() -> PromiseWithResolvers<T, E> {
    let (sender, receiver) = oneshot::channel();
    PromiseWithResolvers {
        sender: Arc::new(Mutex::new(Some(sender))),
        receiver: Arc::new(Mutex::new(Some(receiver))),
    }
}
