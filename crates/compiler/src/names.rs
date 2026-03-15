use rustc_hash::FxHashSet;
use std::sync::Arc;

pub(crate) type Name = Arc<str>;
pub(crate) type NameSet = FxHashSet<Name>;

#[derive(Clone, Copy, Debug)]
pub(crate) struct NameMark(usize);

#[derive(Clone, Debug, Default)]
pub(crate) struct NameStack(Vec<Name>);

impl NameStack {
    pub(crate) fn as_slice(&self) -> &[Name] {
        &self.0
    }

    pub(crate) fn from_items<I>(items: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Name>,
    {
        Self(items.into_iter().map(Into::into).collect())
    }

    pub(crate) fn mark(&self) -> NameMark {
        NameMark(self.0.len())
    }

    pub(crate) fn reset(&mut self, mark: NameMark) {
        self.0.truncate(mark.0);
    }

    pub(crate) fn with_frame<T>(
        &mut self,
        extend: impl FnOnce(&mut Self),
        visit: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let mark = self.mark();
        extend(self);
        let result = visit(self);
        self.reset(mark);
        result
    }

    pub(crate) fn push(&mut self, name: Name) {
        self.0.push(name);
    }

    pub(crate) fn pop(&mut self) {
        let _ = self.0.pop();
    }

    pub(crate) fn extend<I>(&mut self, items: I)
    where
        I: IntoIterator<Item = Name>,
    {
        self.0.extend(items);
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.0.iter().any(|item| item.as_ref() == name)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct OrderedNames {
    names: Vec<Name>,
    seen: NameSet,
}

impl OrderedNames {
    pub(crate) fn as_slice(&self) -> &[Name] {
        &self.names
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub(crate) fn retain<F>(&mut self, mut keep: F)
    where
        F: FnMut(&Name) -> bool,
    {
        self.names.retain(|name| keep(name));
        self.seen = self.names.iter().cloned().collect();
    }

    pub(crate) fn into_boxed_slice(self) -> Box<[Name]> {
        self.names.into_boxed_slice()
    }

    pub(crate) fn into_parts(self) -> (Box<[Name]>, NameSet) {
        (self.names.into_boxed_slice(), self.seen)
    }
}

impl Extend<Name> for OrderedNames {
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = Name>,
    {
        for name in iter {
            if self.seen.insert(name.clone()) {
                self.names.push(name);
            }
        }
    }
}

impl IntoIterator for OrderedNames {
    type Item = Name;
    type IntoIter = std::vec::IntoIter<Name>;

    fn into_iter(self) -> Self::IntoIter {
        self.names.into_iter()
    }
}
