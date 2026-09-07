//! Numeric types that cross the IPC boundary.
//!
//! Specta refuses to export a bare `u64`, and it is right to: JavaScript numbers are
//! exact only to 2^53, so a `u64` field silently loses precision above that. Rather
//! than cast at a dozen call sites, the judgement is recorded once, in a type.
//!
//! Both wrappers serialise transparently — serde writes a JSON number and `JSON.parse`
//! reads a JavaScript `number`, which is exactly what the generated TypeScript says.
//! The precision ceiling is 9,007,199,254,740,992: that many bytes is 8 PiB, and that
//! many sequenced updates is nine million years at the 10 Hz progress budget. If Relay
//! ever transfers an 8 PiB file, this comment is the least of its problems.

use std::fmt;

use serde::{Deserialize, Serialize};
use specta::datatype::DataType;

/// A count of bytes, or a rate in bytes per second.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Bytes(pub u64);

/// A monotonic position in the engine's update stream.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Seq(pub u64);

/// A queue position. Gap-based (1024, 2048, …) so that reordering a row rewrites one
/// number instead of renumbering the queue, and signed so a job can move ahead of the
/// first one without a rewrite.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Order(pub i64);

macro_rules! js_number {
    ($ty:ty, $inner:ty, $ts:ty) => {
        impl specta::Type for $ty {
            fn definition(types: &mut specta::Types) -> DataType {
                // `u32`/`i32` is simply specta's spelling of TypeScript `number`; the
                // wire value is still the full 64-bit integer. See the module comment.
                <$ts as specta::Type>::definition(types)
            }
        }

        impl From<$inner> for $ty {
            fn from(value: $inner) -> Self {
                Self(value)
            }
        }

        impl From<$ty> for $inner {
            fn from(value: $ty) -> Self {
                value.0
            }
        }

        impl $ty {
            pub const fn get(self) -> $inner {
                self.0
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

js_number!(Bytes, u64, u32);
js_number!(Seq, u64, u32);
js_number!(Order, i64, i32);

impl Bytes {
    pub const ZERO: Bytes = Bytes(0);
}

impl std::ops::Add for Bytes {
    type Output = Bytes;
    fn add(self, rhs: Bytes) -> Bytes {
        Bytes(self.0.saturating_add(rhs.0))
    }
}

impl std::ops::AddAssign for Bytes {
    fn add_assign(&mut self, rhs: Bytes) {
        self.0 = self.0.saturating_add(rhs.0);
    }
}

impl Seq {
    pub const ZERO: Seq = Seq(0);

    /// Advance and return the new position.
    pub fn advance(&mut self) -> Seq {
        self.0 += 1;
        *self
    }
}
