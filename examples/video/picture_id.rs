use num::Bounded;
use std::ops::{Add, Sub};
use num::traits::WrappingAdd;
use web_test::utils::Unsigned;

#[derive(Debug, Clone, Copy, Ord, PartialOrd, Eq, PartialEq)]
pub struct FullPictureId {
    inner: u16
}

impl Bounded for FullPictureId {
    fn min_value() -> Self {
        FullPictureId { inner: 0 }
    }

    fn max_value() -> Self {
        FullPictureId { inner: 0x7FFF }
    }
}

impl From<u8> for FullPictureId {
    fn from(v: u8) -> Self {
        FullPictureId { inner: v as u16 }
    }
}

impl From<u16> for FullPictureId {
    fn from(v: u16) -> Self {
        FullPictureId { inner: v & 0x7FFF }
    }
}

impl From<u32> for FullPictureId {
    fn from(v: u32) -> Self {
        FullPictureId { inner: v as u16 & 0x7FFF }
    }
}

impl From<i32> for FullPictureId {
    fn from(v: i32) -> Self {
        FullPictureId { inner: v as u16 & 0x7FFF }
    }
}

impl Add for FullPictureId {
    type Output = FullPictureId;

    fn add(self, rhs: Self) -> Self::Output {
        FullPictureId { inner: (self.inner + rhs.inner) & 0x7FFF }
    }
}

impl WrappingAdd for FullPictureId {
    fn wrapping_add(&self, v: &Self) -> Self {
        FullPictureId { inner: (self.inner + v.inner) & 0x7FFF }
    }
}

impl Unsigned for FullPictureId {}

impl Sub for FullPictureId {
    type Output = FullPictureId;

    fn sub(self, rhs: Self) -> Self::Output {
        FullPictureId { inner: self.inner - rhs.inner }
    }
}

#[derive(Debug, Clone, Copy, Ord, PartialOrd, Eq, PartialEq)]
pub struct ShortPictureId {
    inner: u8
}

impl Bounded for ShortPictureId {
    fn min_value() -> Self {
        ShortPictureId { inner: 0 }
    }

    fn max_value() -> Self {
        ShortPictureId { inner: 0x7F }
    }
}

impl From<u8> for ShortPictureId {
    fn from(v: u8) -> Self {
        ShortPictureId { inner: v }
    }
}

impl From<u16> for ShortPictureId {
    fn from(v: u16) -> Self {
        ShortPictureId { inner: v as u8 & 0x7F }
    }
}

impl From<u32> for ShortPictureId {
    fn from(v: u32) -> Self {
        ShortPictureId { inner: v as u8 & 0x7F }
    }
}

impl From<i32> for ShortPictureId {
    fn from(v: i32) -> Self {
        ShortPictureId { inner: v as u8 & 0x7F }
    }
}

impl Add for ShortPictureId {
    type Output = ShortPictureId;

    fn add(self, rhs: Self) -> Self::Output {
        ShortPictureId { inner: (self.inner + rhs.inner) & 0x7F }
    }
}

impl WrappingAdd for ShortPictureId {
    fn wrapping_add(&self, v: &Self) -> Self {
        ShortPictureId { inner: (self.inner + v.inner) & 0x7F }
    }
}

impl Unsigned for ShortPictureId {}

impl Sub for ShortPictureId {
    type Output = ShortPictureId;

    fn sub(self, rhs: Self) -> Self::Output {
        ShortPictureId { inner: self.inner - rhs.inner }
    }
}