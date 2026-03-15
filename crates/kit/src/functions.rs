pub struct OnceFn<T, F> {
    callback: Option<F>,
    result: Option<T>,
}

impl<T, F> OnceFn<T, F>
where
    F: FnOnce() -> T,
{
    pub fn call(&mut self) -> &T {
        if self.result.is_none() {
            let callback = self
                .callback
                .take()
                .expect("once callback should still exist before first call");
            self.result = Some(callback());
        }

        self.result
            .as_ref()
            .expect("once callback should have produced a value")
    }
}

pub fn once<T, F>(callback: F) -> OnceFn<T, F>
where
    F: FnOnce() -> T,
{
    OnceFn {
        callback: Some(callback),
        result: None,
    }
}
