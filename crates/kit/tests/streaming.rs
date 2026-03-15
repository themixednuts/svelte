use svelte_kit::create_async_iterator;

#[tokio::test]
async fn async_iterator_handles_fast_consecutive_resolutions() {
    let queue = create_async_iterator();
    queue.add(async { 1 });
    queue.add(async { 2 });

    let actual = queue.collect_mapped(|n| n * 10).await;
    assert_eq!(actual, vec![10, 20]);
}
