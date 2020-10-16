use std::ops::{Sub, Add};
use num::{Bounded};
use num::traits::{WrappingAdd, WrappingSub};

pub trait Unsigned {}
impl Unsigned for u8 {}
impl Unsigned for u16 {}
impl Unsigned for u32 {}
impl Unsigned for u64 {}
#[cfg(has_i128)]
impl Unsigned for u128 {}

pub trait SequenceNumberBase = Unsigned + Clone + Copy + WrappingAdd + Sub<Output=Self> + Bounded + Ord + From<u8> + Sized;

/// A wrapper around an u16 to represent a packet it
#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub struct SequenceNumber<T: SequenceNumberBase> {
    pub packet_id: T
}

impl<T: Bounded + SequenceNumberBase> SequenceNumber<T> {
    pub fn new(packet_id: T) -> Self {
        SequenceNumber { packet_id }
    }

    pub fn is_less(&self, other: &Self, clipping_window: Option<T>) -> bool {
        if let Some(window) = clipping_window {
            if self.packet_id < window {
                self.packet_id < other.packet_id && other.packet_id < T::max_value() - window
            } else if self.packet_id > T::max_value() - window {
                self.packet_id < other.packet_id || self.packet_id.wrapping_add(&window) >= other.packet_id
            } else {
                self.packet_id < other.packet_id
            }
        } else {
            self.packet_id < other.packet_id
        }
    }

    pub fn difference(&self, other: &Self, clipping_window: Option<T>) -> T {
        if let Some(window) = clipping_window {
            if self.packet_id < window {
                return if other.packet_id > T::max_value() - window {
                    (T::max_value() - other.packet_id) + self.packet_id + T::from(1)
                } else if other.packet_id > self.packet_id {
                    other.packet_id - self.packet_id
                } else {
                    self.packet_id - other.packet_id
                }
            } else if other.packet_id < window {
                return if self.packet_id > T::max_value() - window {
                    (T::max_value() - self.packet_id) + other.packet_id + T::from(1)
                } else if self.packet_id > other.packet_id {
                    self.packet_id - other.packet_id
                } else {
                    other.packet_id - self.packet_id
                }
            }
        }

        if self.packet_id > other.packet_id {
            self.packet_id - other.packet_id
        } else {
            other.packet_id - self.packet_id
        }
    }
}

impl<T: SequenceNumberBase> Add<T> for SequenceNumber<T> {
    type Output = SequenceNumber<T>;

    fn add(self, rhs: T) -> Self::Output {
        SequenceNumber::new(self.packet_id.wrapping_add(&rhs))
    }
}

impl<T: SequenceNumberBase + WrappingSub> Sub<T> for SequenceNumber<T> {
    type Output = SequenceNumber<T>;

    fn sub(self, rhs: T) -> Self::Output {
        SequenceNumber::new(self.packet_id.wrapping_sub(&rhs))
    }
}

impl<T: SequenceNumberBase> PartialEq<T> for SequenceNumber<T> {
    fn eq(&self, other: &T) -> bool {
        self.packet_id == *other
    }
}

impl<T: SequenceNumberBase> From<T> for SequenceNumber<T> {
    fn from(id: T) -> Self {
        SequenceNumber::new(id)
    }
}

/*
impl<T: SequenceNumberBase> From<SequenceNumber<T>> for T {
    fn from(id: SequenceNumber<T>) -> T {
        id.packet_id
    }
}
*/

#[cfg(test)]
mod tests_u16 {
    use crate::utils::SequenceNumber;

    fn test_packet_id(a: u16, b: u16, result: bool, clipping_window: Option<u16>) {
        let a = SequenceNumber { packet_id: a };
        let b = SequenceNumber { packet_id: b };
        assert_eq!(a.is_less(&b, clipping_window), result);
    }

    fn test_packet_difference(a: u16, b: u16, expected: u16, clipping_window: Option<u16>) {
        let a = SequenceNumber { packet_id: a };
        let b = SequenceNumber { packet_id: b };
        assert_eq!(a.difference(&b, clipping_window), expected);
        assert_eq!(b.difference(&a, clipping_window), expected);
    }

    #[test]
    fn packet_id_is_less_basic() {
        test_packet_id(2, 3, true, None);
        test_packet_id(4, 3, false, None);
    }

    #[test]
    fn packet_id_is_less_clipping() {
        test_packet_id(0xFFFF, 0, false, None);
        test_packet_id(0xFFFF, 1, false, None);
        test_packet_id(0xFFFF, 2, false, None);
        test_packet_id(0xFFFF, 2, true, Some(4));
        test_packet_id(0xFFFF, 2, false, Some(2));
        test_packet_id(2, 0xFFFF, false, Some(4));

        for i in 1..0x2Fu16 {
            test_packet_id(i.wrapping_add(0xFFF0), i.wrapping_add(0xFFF1), true, Some(2));
            test_packet_id(i.wrapping_add(0xFFF0), i.wrapping_add(0xFFF5), true, Some(6));

            test_packet_id(i.wrapping_add(0xFFF6), i.wrapping_add(0xFFF0), false, Some(6));
            test_packet_id(i.wrapping_add(0xFFF0), i.wrapping_add(0xFFF6), true, Some(6));
        }
    }

    #[test]
    fn packet_id_difference() {
        test_packet_difference(0, 0, 0, None);
        test_packet_difference(0xFFFF, 0, 0xFFFF, None);
        test_packet_difference(0xFFFF, 0, 1, Some(1));

        for i in 0..0xFFu16 {
            test_packet_difference(0xFF8F_u16.wrapping_add(i), 0xFF9F_u16.wrapping_add(i), 16, Some(16));
        }
    }
}

#[cfg(test)]
mod tests_u32 {
    use crate::utils::SequenceNumber;

    fn test_packet_id(a: u32, b: u32, result: bool, clipping_window: Option<u32>) {
        let a = SequenceNumber { packet_id: a };
        let b = SequenceNumber { packet_id: b };
        assert_eq!(a.is_less(&b, clipping_window), result);
    }

    fn test_packet_difference(a: u32, b: u32, expected: u32, clipping_window: Option<u32>) {
        let a = SequenceNumber { packet_id: a };
        let b = SequenceNumber { packet_id: b };
        assert_eq!(a.difference(&b, clipping_window), expected);
        assert_eq!(b.difference(&a, clipping_window), expected);
    }

    #[test]
    fn packet_id_is_less_basic() {
        test_packet_id(2, 3, true, None);
        test_packet_id(4, 3, false, None);
    }

    #[test]
    fn packet_id_is_less_clipping() {
        test_packet_id(0xFFFF_FFFF, 0, false, None);
        test_packet_id(0xFFFF_FFFF, 1, false, None);
        test_packet_id(0xFFFF_FFFF, 2, false, None);
        test_packet_id(0xFFFF_FFFF, 2, true, Some(4));
        test_packet_id(0xFFFF_FFFF, 2, false, Some(2));
        test_packet_id(2, 0xFFFF_FFFF, false, Some(4));

        for i in 1..0x2Fu32 {
            test_packet_id(i.wrapping_add(0xFFFF_FFF0), i.wrapping_add(0xFFFF_FFF1), true, Some(2));
            test_packet_id(i.wrapping_add(0xFFFF_FFF0), i.wrapping_add(0xFFFF_FFF5), true, Some(6));

            test_packet_id(i.wrapping_add(0xFFFF_FFF6), i.wrapping_add(0xFFFF_FFF0), false, Some(6));
            test_packet_id(i.wrapping_add(0xFFFF_FFF0), i.wrapping_add(0xFFFF_FFF6), true, Some(6));
        }
    }

    #[test]
    fn packet_id_difference() {
        test_packet_difference(0, 0, 0, None);
        test_packet_difference(0xFFFF_FFFF, 0, 0xFFFF_FFFF, None);
        test_packet_difference(0xFFFF_FFFF, 0, 1, Some(1));

        for i in 0..0xFFu32 {
            test_packet_difference(0xFFFF_FF8F_u32.wrapping_add(i), 0xFFFF_FF9F_u32.wrapping_add(i), 16, Some(16));
        }
    }
}

#[cfg(test)]
mod tests_custum {
    use crate::utils::{SequenceNumber, Unsigned};
    use num::traits::WrappingAdd;
    use std::ops::{Add, Sub};
    use num::{Bounded};

    #[derive(Debug, Clone, Copy, Ord, PartialOrd, Eq, PartialEq)]
    struct U15 {
        inner: u16
    }

    impl Bounded for U15 {
        fn min_value() -> Self {
            U15{ inner: 0 }
        }

        fn max_value() -> Self {
            U15{ inner: 0x7FFF }
        }
    }

    impl From<u8> for U15 {
        fn from(v: u8) -> Self {
            U15{ inner: v as u16 }
        }
    }

    impl From<u16> for U15 {
        fn from(v: u16) -> Self {
            U15{ inner: v & 0x7FFF }
        }
    }

    impl From<u32> for U15 {
        fn from(v: u32) -> Self {
            U15{ inner: v as u16 & 0x7FFF }
        }
    }

    impl From<i32> for U15 {
        fn from(v: i32) -> Self {
            U15{ inner: v as u16 & 0x7FFF }
        }
    }

    impl Add for U15 {
        type Output = U15;

        fn add(self, rhs: Self) -> Self::Output {
            U15 { inner: (self.inner + rhs.inner) & 0x7FFF }
        }
    }

    impl WrappingAdd for U15 {
        fn wrapping_add(&self, v: &Self) -> Self {
            U15 { inner: (self.inner + v.inner) & 0x7FFF }
        }
    }

    impl Unsigned for U15 {}

    impl Sub for U15 {
        type Output = U15;

        fn sub(self, rhs: Self) -> Self::Output {
            U15{ inner: self.inner - rhs.inner }
        }
    }

    fn test_packet_id(a: U15, b: U15, result: bool, clipping_window: Option<U15>) {
        let a = SequenceNumber { packet_id: a };
        let b = SequenceNumber { packet_id: b };
        assert_eq!(a.is_less(&b, clipping_window), result);
    }

    fn test_packet_difference(a: U15, b: U15, expected: U15, clipping_window: Option<U15>) {
        let a = SequenceNumber { packet_id: a };
        let b = SequenceNumber { packet_id: b };
        assert_eq!(a.difference(&b, clipping_window), expected);
        assert_eq!(b.difference(&a, clipping_window), expected);
    }

    #[test]
    fn packet_id_is_less_basic() {
        test_packet_id(2.into(), 3.into(), true, None);
        test_packet_id(4.into(), 3.into(), false, None);
    }

    #[test]
    fn packet_id_is_less_clipping() {
        test_packet_id(0x7FFF.into(), 0.into(), false, None);
        test_packet_id(0x7FFF.into(), 1.into(), false, None);
        test_packet_id(0x7FFF.into(), 2.into(), false, None);
        test_packet_id(0x7FFF.into(), 2.into(), true, Some(4.into()));
        test_packet_id(0x7FFF.into(), 2.into(), false, Some(2.into()));
        test_packet_id(2.into(), 0x7FFF.into(), false, Some(4.into()));

        for i in 1..0x2Fu16 {
            let i = U15::from(i);
            test_packet_id(i.wrapping_add(&0x7FF0.into()), i.wrapping_add(&0x7FF1.into()), true, Some(2.into()));
            test_packet_id(i.wrapping_add(&0x7FF0.into()), i.wrapping_add(&0x7FF5.into()), true, Some(6.into()));

            test_packet_id(i.wrapping_add(&0x7FF6.into()), i.wrapping_add(&0x7FF0.into()), false, Some(6.into()));
            test_packet_id(i.wrapping_add(&0x7FF0.into()), i.wrapping_add(&0x7FF6.into()), true, Some(6.into()));
        }
    }

    #[test]
    fn packet_id_difference() {
        test_packet_difference(0.into(), 0.into(), 0.into(), None);
        test_packet_difference(0x7FFF.into(), 0.into(), 0x7FFF.into(), None);
        test_packet_difference(0x7FFF.into(), 0.into(), 1.into(), Some(1.into()));

        for i in 0..0xFFu16 {
            let i = &U15::from(i);
            test_packet_difference(i.wrapping_add(&0x7F8F.into()), i.wrapping_add(&0x7F9F.into()), 16.into(), Some(16.into()));
        }
    }
}