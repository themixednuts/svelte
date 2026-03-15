use std::future::Future;
use std::sync::Mutex;

use tokio::task::JoinHandle;

pub struct AsyncIterator<T> {
    tasks: Mutex<Vec<JoinHandle<T>>>,
}

impl<T> Default for AsyncIterator<T> {
    fn default() -> Self {
        Self {
            tasks: Mutex::new(Vec::new()),
        }
    }
}

pub fn create_async_iterator<T>() -> AsyncIterator<T> {
    AsyncIterator::default()
}

impl<T: Send + 'static> AsyncIterator<T> {
    pub fn add<F>(&self, future: F)
    where
        F: Future<Output = T> + Send + 'static,
    {
        let handle = tokio::spawn(future);
        self.tasks
            .lock()
            .expect("async iterator task queue should lock")
            .push(handle);
    }

    pub async fn collect_mapped<U, F>(&self, mut transform: F) -> Vec<U>
    where
        F: FnMut(T) -> U,
    {
        let handles = {
            let mut tasks = self
                .tasks
                .lock()
                .expect("async iterator task queue should lock");
            std::mem::take(&mut *tasks)
        };

        let mut values = Vec::with_capacity(handles.len());
        for handle in handles {
            let value = handle.await.expect("async iterator task should complete");
            values.push(transform(value));
        }
        values
    }
}
