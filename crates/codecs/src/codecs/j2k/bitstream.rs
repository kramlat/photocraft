//! Bit-level I/O helpers for JPEG 2000 codestream reading and writing.
//!
//! Provides [`BitReader`] and [`BitWriter`] for bit-granularity access
//! to byte buffers, plus [`ByteReader`] and [`ByteWriter`] for big-endian
//! multi-byte primitives.

use std::io::{self, Read, Write};

// ---------------------------------------------------------------------------
// BitReader
// ---------------------------------------------------------------------------

/// Reads bits from a byte stream (MSB first).
pub struct BitReader<R> {
    inner: R,
    buf: u32,
    bits: u32,
}

impl<R: Read> BitReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            buf: 0,
            bits: 0,
        }
    }

    /// Read a single bit (0 or 1).
    pub fn read_bit(&mut self) -> io::Result<u32> {
        if self.bits == 0 {
            self.fill()?;
        }
        self.bits -= 1;
        Ok((self.buf >> self.bits) & 1)
    }

    /// Read `n` bits (n <= 25) and return them right-justified.
    pub fn read_bits(&mut self, n: u32) -> io::Result<u32> {
        while self.bits < n {
            self.fill()?;
        }
        self.bits -= n;
        Ok((self.buf >> self.bits) & ((1 << n) - 1))
    }

    /// Discard buffered bits, aligning to byte boundary.
    pub fn align(&mut self) {
        self.bits = 0;
        self.buf = 0;
    }

    fn fill(&mut self) -> io::Result<()> {
        let mut byte = [0u8; 1];
        self.inner.read_exact(&mut byte)?;
        self.buf = (self.buf << 8) | u32::from(byte[0]);
        self.bits += 8;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// BitWriter
// ---------------------------------------------------------------------------

/// Writes bits to a byte stream (MSB first).
pub struct BitWriter<W> {
    inner: W,
    buf: u32,
    bits: u32,
}

impl<W: Write> BitWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buf: 0,
            bits: 0,
        }
    }

    /// Write a single bit (0 or 1).
    pub fn write_bit(&mut self, bit: u32) -> io::Result<()> {
        self.buf = (self.buf << 1) | (bit & 1);
        self.bits += 1;
        if self.bits >= 8 {
            self.flush_byte()?;
        }
        Ok(())
    }

    /// Write the lowest `n` bits of `val` (n <= 25).
    pub fn write_bits(&mut self, val: u32, n: u32) -> io::Result<()> {
        self.buf = (self.buf << n) | (val & ((1 << n) - 1));
        self.bits += n;
        while self.bits >= 8 {
            self.flush_byte()?;
        }
        Ok(())
    }

    /// Pad remaining bits with zeros and flush to the underlying writer.
    pub fn flush(&mut self) -> io::Result<()> {
        if self.bits > 0 {
            let padding = 8 - self.bits;
            self.buf <<= padding;
            self.bits = 8;
            self.flush_byte()?;
        }
        self.inner.flush()
    }

    /// Pad remaining bits with ones (JPEG 2000 convention) and flush.
    pub fn flush_with_stuffing(&mut self) -> io::Result<()> {
        if self.bits > 0 {
            let padding = 8 - self.bits;
            self.buf = (self.buf << padding) | ((1 << padding) - 1);
            self.bits = 8;
            self.flush_byte()?;
        }
        self.inner.flush()
    }

    fn flush_byte(&mut self) -> io::Result<()> {
        let shift = self.bits - 8;
        let c = ((self.buf >> shift) & 0xFF) as u8;
        self.bits = shift;
        self.buf &= (1 << shift) - 1;
        self.inner.write_all(&[c])
    }

    /// Return a reference to the inner writer.
    pub fn inner(&self) -> &W {
        &self.inner
    }

    /// Consume the `BitWriter`, returning the inner writer.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

// ---------------------------------------------------------------------------
// ByteReader -- big-endian multi-byte reads from a slice
// ---------------------------------------------------------------------------

/// Reads big-endian primitives from a byte slice with a cursor.
pub struct ByteReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Current read position.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Remaining unread bytes.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn read_u8(&mut self) -> io::Result<u8> {
        if self.pos >= self.data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "unexpected end of data",
            ));
        }
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    pub fn read_u16(&mut self) -> io::Result<u16> {
        let hi = self.read_u8()? as u16;
        let lo = self.read_u8()? as u16;
        Ok((hi << 8) | lo)
    }

    pub fn read_u32(&mut self) -> io::Result<u32> {
        let mut val = 0u32;
        for _ in 0..4 {
            val = (val << 8) | self.read_u8()? as u32;
        }
        Ok(val)
    }

    pub fn read_bytes(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "unexpected end of data",
            ));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    pub fn skip(&mut self, n: usize) -> io::Result<()> {
        if self.pos + n > self.data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "cannot skip past end of data",
            ));
        }
        self.pos += n;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ByteWriter -- big-endian multi-byte writes to a Vec
// ---------------------------------------------------------------------------

/// Writes big-endian primitives into a growable byte buffer.
pub struct ByteWriter {
    buf: Vec<u8>,
}

impl ByteWriter {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(4096),
        }
    }

    pub fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn write_u16(&mut self, v: u16) {
        self.buf.push((v >> 8) as u8);
        self.buf.push(v as u8);
    }

    pub fn write_u32(&mut self, v: u32) {
        self.buf.push((v >> 24) as u8);
        self.buf.push((v >> 16) as u8);
        self.buf.push((v >> 8) as u8);
        self.buf.push(v as u8);
    }

    pub fn write_bytes(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Return the accumulated bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// Current length of the buffer.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}
