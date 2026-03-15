use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use tokio::sync::{Mutex, oneshot};

use svelte_kit::queue;

async fn wait_for_count(count: &AtomicUsize, expected: usize) {
    while count.load(Ordering::SeqCst) < expected {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn queue_add_resolves_to_the_correct_value() {
    let q = queue::<i32, String>(1);

    let value = q
        .add(|| async { Ok(42) })
        .await
        .expect("queue open")
        .await
        .expect("task success");

    assert_eq!(value, 42);
}

#[tokio::test]
async fn queue_add_rejects_if_task_rejects() {
    let q = queue::<i32, String>(1);

    let error = q
        .add(|| async { Err("nope".to_string()) })
        .await
        .expect("queue open")
        .await
        .expect_err("task should fail");

    assert_eq!(error, "nope");
}

#[tokio::test]
async fn queue_starts_tasks_in_sequence() {
    let q = queue::<(), String>(2);
    let started = Arc::new(Mutex::new(vec![false, false, false, false]));
    let finished = Arc::new(Mutex::new(vec![false, false, false, false]));
    let started_count = Arc::new(AtomicUsize::new(0));

    let mut senders = Vec::new();
    let mut handles = Vec::new();

    for i in 0..4 {
        let started = Arc::clone(&started);
        let finished = Arc::clone(&finished);
        let started_count = Arc::clone(&started_count);
        let (sender, receiver) = oneshot::channel::<()>();
        senders.push(sender);
        handles.push(
            q.add(move || async move {
                {
                    let mut started = started.lock().await;
                    started[i] = true;
                }
                started_count.fetch_add(1, Ordering::SeqCst);
                let _ = receiver.await;
                {
                    let mut finished = finished.lock().await;
                    finished[i] = true;
                }
                Ok(())
            })
            .await
            .expect("queue open"),
        );
    }

    wait_for_count(&started_count, 2).await;
    assert_eq!(&*started.lock().await, &[true, true, false, false]);
    assert_eq!(&*finished.lock().await, &[false, false, false, false]);

    let _ = senders.remove(0).send(());
    handles.remove(0).await.expect("task success");
    wait_for_count(&started_count, 3).await;
    assert_eq!(&*started.lock().await, &[true, true, true, false]);
    assert_eq!(&*finished.lock().await, &[true, false, false, false]);

    let _ = senders.remove(0).send(());
    handles.remove(0).await.expect("task success");
    wait_for_count(&started_count, 4).await;
    assert_eq!(&*started.lock().await, &[true, true, true, true]);
    assert_eq!(&*finished.lock().await, &[true, true, false, false]);

    let _ = senders.remove(0).send(());
    let _ = senders.remove(0).send(());
    for handle in handles {
        handle.await.expect("task success");
    }
    q.done().await.expect("queue success");

    assert_eq!(&*finished.lock().await, &[true, true, true, true]);
}

#[tokio::test]
async fn queue_add_fails_if_queue_is_finished() {
    let q = queue::<(), String>(1);
    q.add(|| async { Ok(()) }).await.expect("queue open");

    q.done().await.expect("queue success");
    assert!(q.add(|| async { Ok(()) }).await.is_err());
}

#[tokio::test]
async fn queue_done_resolves_if_nothing_was_added() {
    let q = queue::<(), String>(100);
    q.done().await.expect("empty queue should resolve");
}

#[tokio::test]
async fn queue_allows_adding_while_done_is_waiting_for_drain() {
    let q = queue::<usize, String>(1);
    let (first_sender, first_receiver) = oneshot::channel::<()>();

    let first = q
        .add(move || async move {
            let _ = first_receiver.await;
            Ok(1)
        })
        .await
        .expect("queue open");

    let done = {
        let q = q.clone();
        tokio::spawn(async move { q.done().await })
    };

    tokio::task::yield_now().await;

    let second = q
        .add(|| async { Ok(2) })
        .await
        .expect("queue should still accept tasks while draining");

    let _ = first_sender.send(());
    assert_eq!(first.await.expect("first success"), 1);
    assert_eq!(second.await.expect("second success"), 2);
    done.await.expect("join done").expect("queue success");
}

#[tokio::test]
async fn queue_done_rejects_if_task_rejects() {
    let q = queue::<(), String>(1);

    let handle = q
        .add(|| async { Err("nope".to_string()) })
        .await
        .expect("queue open");
    let error = q.done().await.expect_err("done should fail");
    let task_error = handle.await.expect_err("task should fail");

    assert_eq!(error, "nope");
    assert_eq!(task_error, "nope");
}
