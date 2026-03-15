use std::{sync::Arc, thread};

use crate::{ForkError, Result};

pub fn forked<T, U, F>(callback: F) -> impl Fn(T) -> Result<U>
where
    T: Send + 'static,
    U: Send + 'static,
    F: Fn(T) -> U + Send + Sync + 'static,
{
    let callback = Arc::new(callback);
    move |value| {
        let callback = Arc::clone(&callback);
        thread::spawn(move || callback(value))
            .join()
            .map_err(|panic| {
                if let Some(message) = panic.downcast_ref::<&str>() {
                    ForkError::PanicMessage {
                        message: (*message).to_string(),
                    }
                    .into()
                } else if let Some(message) = panic.downcast_ref::<String>() {
                    ForkError::PanicMessage {
                        message: message.clone(),
                    }
                    .into()
                } else {
                    ForkError::ThreadPanicked.into()
                }
            })
    }
}
