use std::ops::{Sub, Add};

/// A wrapper around an u16 to represent a packet it
#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub struct PacketId {
    pub packet_id: u16
}

impl PacketId {
    pub fn new(packet_id: u16) -> PacketId {
        PacketId{ packet_id }
    }

    pub fn is_less(&self, other: &Self, clipping_window: Option<u16>) -> bool {
        if let Some(window) = clipping_window {
            if self.packet_id < window {
                self.packet_id < other.packet_id && other.packet_id < 0xFFFF - window
            } else if self.packet_id > 0xFFFF - window {
                self.packet_id < other.packet_id || self.packet_id.wrapping_add(window) >= other.packet_id
            } else {
                self.packet_id < other.packet_id
            }
        } else {
            self.packet_id < other.packet_id
        }
    }

    pub fn difference(&self, other: &Self, clipping_window: Option<u16>) -> u16 {
        if let Some(window) = clipping_window {
            if self.packet_id < window {
                return if other.packet_id > 0xFFFF - window {
                    (0xFFFF - other.packet_id) + self.packet_id + 1
                } else if other.packet_id > self.packet_id {
                    other.packet_id - self.packet_id
                } else {
                    self.packet_id - other.packet_id
                }
            } else if other.packet_id < window {
                return if self.packet_id > 0xFFFF - window {
                    (0xFFFF - self.packet_id) + other.packet_id + 1
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

impl Add<u16> for PacketId {
    type Output = PacketId;

    fn add(self, rhs: u16) -> Self::Output {
        PacketId::new(self.packet_id.wrapping_add(rhs))
    }
}

impl Sub<u16> for PacketId {
    type Output = PacketId;

    fn sub(self, rhs: u16) -> Self::Output {
        PacketId::new(self.packet_id.wrapping_sub(rhs))
    }
}

impl PartialEq<u16> for PacketId {
    fn eq(&self, other: &u16) -> bool {
        self.packet_id == *other
    }
}

impl From<u16> for PacketId {
    fn from(id: u16) -> Self {
        PacketId::new(id)
    }
}

impl From<PacketId> for u16 {
    fn from(id: PacketId) -> Self {
        id.packet_id
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::PacketId;

    fn test_packet_id(a: u16, b: u16, result: bool, clipping_window: Option<u16>) {
        let a = PacketId{ packet_id: a };
        let b = PacketId{ packet_id: b };
        assert_eq!(a.is_less(&b, clipping_window), result);
    }

    fn test_packet_difference(a: u16, b: u16, expected: u16, clipping_window: Option<u16>) {
        let a = PacketId{ packet_id: a };
        let b = PacketId{ packet_id: b };
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