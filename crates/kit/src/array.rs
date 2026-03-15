pub fn compact<T>(values: impl IntoIterator<Item = Option<T>>) -> Vec<T> {
    values.into_iter().flatten().collect()
}
