use std::sync::{Mutex, mpsc};
use std::thread;

pub(crate) fn map_in_parallel<T, U, F>(items: Vec<T>, worker: F) -> Vec<U>
where
    T: Send,
    U: Send,
    F: Fn(T) -> U + Sync,
{
    if items.len() <= 1 {
        return items.into_iter().map(worker).collect();
    }

    let worker_count = thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .min(items.len());
    if worker_count <= 1 {
        return items.into_iter().map(worker).collect();
    }

    let job_count = items.len();
    let jobs = Mutex::new(items.into_iter().enumerate());
    let worker_ref = &worker;
    let (tx, rx) = mpsc::channel();

    thread::scope(|scope| {
        for _ in 0..worker_count {
            let tx = tx.clone();
            let jobs = &jobs;
            let worker = worker_ref;
            scope.spawn(move || {
                loop {
                    let next_job = {
                        jobs.lock()
                            .expect("batch compile job queue should not be poisoned")
                            .next()
                    };
                    let Some((index, item)) = next_job else {
                        break;
                    };
                    let output = worker(item);
                    tx.send((index, output))
                        .expect("batch compile receiver should stay alive");
                }
            });
        }
        drop(tx);
    });

    let mut outputs = std::iter::repeat_with(|| None)
        .take(job_count)
        .collect::<Vec<Option<U>>>();
    for (index, output) in rx {
        outputs[index] = Some(output);
    }

    outputs
        .into_iter()
        .map(|output| output.expect("every batch compile job should produce an output"))
        .collect()
}
