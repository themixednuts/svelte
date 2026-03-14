use std::fmt;

use serde::{Deserialize, Serialize};

/// A byte offset into source text, stored as a `u32`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[repr(transparent)]
pub struct BytePos(u32);

impl BytePos {
    /// The zero position.
    pub const ZERO: Self = Self(0);

    /// Create a byte position from a raw `u32` value.
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// Return the position as a `u32`.
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Return the position as a `usize`.
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl TryFrom<usize> for BytePos {
    type Error = &'static str;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u32::try_from(value)
            .map(Self)
            .map_err(|_| "byte position exceeds u32 range")
    }
}

impl From<u32> for BytePos {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl fmt::Display for BytePos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A half-open byte range (`start..end`) in source text.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub struct Span {
    /// Inclusive start position.
    pub start: BytePos,
    /// Exclusive end position.
    pub end: BytePos,
}

impl Span {
    /// A zero-length span at position 0.
    pub const EMPTY: Self = Self {
        start: BytePos::ZERO,
        end: BytePos::ZERO,
    };

    /// Create a span from two byte positions.
    pub const fn new(start: BytePos, end: BytePos) -> Self {
        Self { start, end }
    }

    /// Create a span from `usize` offsets, returning `None` if either
    /// offset exceeds the `u32` range.
    pub fn from_offsets(start: usize, end: usize) -> Option<Self> {
        Some(Self {
            start: BytePos::try_from(start).ok()?,
            end: BytePos::try_from(end).ok()?,
        })
    }

    /// Return the length in bytes.
    pub const fn len(self) -> u32 {
        self.end.as_u32().saturating_sub(self.start.as_u32())
    }

    /// Return `true` if the span covers zero bytes.
    pub const fn is_empty(self) -> bool {
        self.start.as_u32() >= self.end.as_u32()
    }

    /// Return `true` if `pos` falls within this span (inclusive on both ends).
    pub fn contains(self, pos: BytePos) -> bool {
        pos >= self.start && pos <= self.end
    }

    /// Return the smallest span that covers both `self` and `other`.
    pub fn join(self, other: Span) -> Span {
        Span {
            start: if self.start <= other.start {
                self.start
            } else {
                other.start
            },
            end: if self.end >= other.end {
                self.end
            } else {
                other.end
            },
        }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// An opaque identifier for a source file, used to distinguish multiple
/// inputs during batch processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[repr(transparent)]
pub struct SourceId(u32);

impl SourceId {
    /// Create a source identifier from a raw `u32` value.
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// Return the raw `u32` value.
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{BytePos, Span};

    #[test]
    fn span_join_and_len() {
        let a = Span::new(BytePos::new(2), BytePos::new(5));
        let b = Span::new(BytePos::new(4), BytePos::new(10));
        let joined = a.join(b);

        assert_eq!(joined.start.as_u32(), 2);
        assert_eq!(joined.end.as_u32(), 10);
        assert_eq!(joined.len(), 8);
    }
}
