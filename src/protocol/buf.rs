// SPDX-License-Identifier: GPL-3.0-or-later
//! Big-endian reader/writer for the subset of `FriendlyByteBuf` that SVC uses.
//!
//! Minecraft's `FriendlyByteBuf` writes big-endian numbers, a `VarInt` length before
//! byte arrays and strings, and UUIDs as two big-endian `long`s. A `u128` written with
//! `to_be_bytes` has exactly that layout.

use std::fmt;

/// Bytes that do not form a valid value of the requested type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Malformed;

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("malformed SVC packet")
    }
}

impl std::error::Error for Malformed {}

pub type Read<T> = Result<T, Malformed>;

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

#[allow(dead_code)]
impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    fn take(&mut self, n: usize) -> Read<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(Malformed)?;
        if end > self.data.len() {
            return Err(Malformed);
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn fixed<const N: usize>(&mut self) -> Read<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    pub fn u8(&mut self) -> Read<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> Read<bool> {
        Ok(self.u8()? != 0)
    }

    pub fn i16(&mut self) -> Read<i16> {
        Ok(i16::from_be_bytes(self.fixed()?))
    }

    pub fn i32(&mut self) -> Read<i32> {
        Ok(i32::from_be_bytes(self.fixed()?))
    }

    pub fn i64(&mut self) -> Read<i64> {
        Ok(i64::from_be_bytes(self.fixed()?))
    }

    pub fn f32(&mut self) -> Read<f32> {
        Ok(f32::from_be_bytes(self.fixed()?))
    }

    pub fn f64(&mut self) -> Read<f64> {
        Ok(f64::from_be_bytes(self.fixed()?))
    }

    pub fn uuid(&mut self) -> Read<u128> {
        Ok(u128::from_be_bytes(self.fixed()?))
    }

    pub fn varint(&mut self) -> Read<i32> {
        let mut value = 0u32;
        for shift in 0..5 {
            let byte = self.u8()?;
            value |= u32::from(byte & 0x7f) << (shift * 7);
            if byte & 0x80 == 0 {
                return Ok(value as i32);
            }
        }
        Err(Malformed)
    }

    /// A `VarInt` length of at most `max`, then that many bytes.
    pub fn bytes(&mut self, max: usize) -> Read<&'a [u8]> {
        let len = self.varint()?;
        if len < 0 {
            return Err(Malformed);
        }
        let len = len as usize;
        if len > max {
            return Err(Malformed);
        }
        self.take(len)
    }

    /// A `VarInt` byte length, then UTF-8, of at most `max_chars` characters.
    pub fn string(&mut self, max_chars: usize) -> Read<String> {
        // Java's `readUtf(max)` bounds the encoded bytes by `max * 3` in older
        // versions and by `max * 4` in the modern one. Accepting `max * 4`
        // covers both, then the character count is checked exactly.
        let raw = self.bytes(max_chars.saturating_mul(4))?;
        let text = std::str::from_utf8(raw).map_err(|_| Malformed)?;
        if text.chars().count() > max_chars {
            return Err(Malformed);
        }
        Ok(text.to_owned())
    }
}

#[derive(Default, Debug, Clone)]
pub struct Writer {
    pub buf: Vec<u8>,
}

#[allow(dead_code)]
impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn u8(&mut self, value: u8) -> &mut Self {
        self.buf.push(value);
        self
    }

    pub fn bool(&mut self, value: bool) -> &mut Self {
        self.u8(u8::from(value))
    }

    pub fn i16(&mut self, value: i16) -> &mut Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn i32(&mut self, value: i32) -> &mut Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn i64(&mut self, value: i64) -> &mut Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn f32(&mut self, value: f32) -> &mut Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn f64(&mut self, value: f64) -> &mut Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn uuid(&mut self, value: u128) -> &mut Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub fn varint(&mut self, value: i32) -> &mut Self {
        let mut value = value as u32;
        while value >= 0x80 {
            self.buf.push((value as u8) | 0x80);
            value >>= 7;
        }
        self.buf.push(value as u8);
        self
    }

    pub fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.varint(bytes.len() as i32);
        self.buf.extend_from_slice(bytes);
        self
    }

    pub fn string(&mut self, text: &str) -> &mut Self {
        self.bytes(text.as_bytes())
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }
}

/// Canonical UUID text, as logs show it.
pub fn uuid_text(uuid: u128) -> String {
    let bytes = uuid.to_be_bytes();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_big_endian() {
        let mut w = Writer::new();
        w.i32(0x0102_0304).i64(0x0102_0304_0506_0708).f32(1.5);
        assert_eq!(&w.buf[..4], &[1, 2, 3, 4]);
        assert_eq!(&w.buf[4..12], &[1, 2, 3, 4, 5, 6, 7, 8]);

        let mut r = Reader::new(&w.buf);
        assert_eq!(r.i32().unwrap(), 0x0102_0304);
        assert_eq!(r.i64().unwrap(), 0x0102_0304_0506_0708);
        assert_eq!(r.f32().unwrap(), 1.5);
    }

    #[test]
    fn varint_roundtrip() {
        for value in [0, 1, 127, 128, 255, 300, 16_384, i32::MAX] {
            let mut w = Writer::new();
            w.varint(value);
            let mut r = Reader::new(&w.buf);
            assert_eq!(r.varint().unwrap(), value);
        }
    }

    #[test]
    fn strings_and_arrays_carry_a_varint_length() {
        let mut w = Writer::new();
        w.string("hello").bytes(&[9, 8, 7]);
        let mut r = Reader::new(&w.buf);
        assert_eq!(r.string(16).unwrap(), "hello");
        assert_eq!(r.bytes(3).unwrap(), &[9, 8, 7]);
    }

    #[test]
    fn a_string_over_the_character_limit_is_malformed() {
        let mut w = Writer::new();
        w.string("abcd");
        let mut r = Reader::new(&w.buf);
        assert!(r.string(3).is_err());
    }

    #[test]
    fn truncated_input_is_malformed() {
        assert!(Reader::new(&[0, 0, 0]).i32().is_err());
        assert!(Reader::new(&[0x80]).varint().is_err());
    }
}
