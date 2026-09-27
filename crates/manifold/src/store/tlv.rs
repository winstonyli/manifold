//! Protobuf-like TLV messages (spec "TLV messages"): each field is a key
//! varint `(field_no << 3) | wire_type`, then a value. Readers skip fields
//! they don't know by wire type. Writers emit fields in ascending number,
//! each at most once, and leave out fields holding their default, so one
//! value has one encoding; that part is up to the caller.

use super::varint::{self, VarintError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    Varint = 0,
    Fixed64 = 1,
    Bytes = 2,
    Fixed32 = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlvError {
    Truncated,
    Overflow,
    /// A wire type other than 0, 1, 2 or 5: its length is unknowable.
    BadWire(u8),
    /// Field number 0, or one past `u32::MAX`.
    BadField,
    BadUtf8,
    /// A value of the wrong wire type or range for its field.
    BadValue,
}

impl From<VarintError> for TlvError {
    fn from(err: VarintError) -> Self {
        match err {
            VarintError::Truncated => TlvError::Truncated,
            VarintError::Overflow => TlvError::Overflow,
        }
    }
}

impl std::fmt::Display for TlvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes into `buf`, cleared, reusing its allocation.
    pub fn reuse(mut buf: Vec<u8>) -> Self {
        buf.clear();
        Self { buf }
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    fn key(&mut self, field: u32, wire: Wire) {
        varint::write(&mut self.buf, (u64::from(field) << 3) | wire as u64);
    }

    pub fn varint(&mut self, field: u32, v: u64) {
        self.key(field, Wire::Varint);
        varint::write(&mut self.buf, v);
    }

    pub fn fixed64(&mut self, field: u32, v: u64) {
        self.key(field, Wire::Fixed64);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn fixed32(&mut self, field: u32, v: u32) {
        self.key(field, Wire::Fixed32);
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bytes(&mut self, field: u32, b: &[u8]) {
        self.key(field, Wire::Bytes);
        varint::write(&mut self.buf, b.len() as u64);
        self.buf.extend_from_slice(b);
    }

    pub fn str(&mut self, field: u32, s: &str) {
        self.bytes(field, s.as_bytes());
    }

    /// Writes `-0.0` as `0.0`, so equal values encode equally. NaN is
    /// never stored; callers drop it first.
    pub fn f32(&mut self, field: u32, v: f32) {
        debug_assert!(!v.is_nan(), "NaN is never stored");
        self.fixed32(field, if v == 0.0 { 0.0f32 } else { v }.to_bits());
    }

    pub fn message(&mut self, field: u32, m: &Writer) {
        self.bytes(field, &m.buf);
    }

    /// Appends an already encoded field, key and value, as a register holds it.
    #[cfg(test)]
    pub fn raw(&mut self, encoded: &[u8]) {
        self.buf.extend_from_slice(encoded);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
}

impl<'a> Value<'a> {
    pub fn u64(self) -> Result<u64, TlvError> {
        match self {
            Value::Varint(v) => Ok(v),
            _ => Err(TlvError::BadValue),
        }
    }

    pub fn u32(self) -> Result<u32, TlvError> {
        u32::try_from(self.u64()?).map_err(|_| TlvError::BadValue)
    }

    pub fn bool(self) -> Result<bool, TlvError> {
        Ok(self.u64()? != 0)
    }

    pub fn fixed64(self) -> Result<u64, TlvError> {
        match self {
            Value::Fixed64(v) => Ok(v),
            _ => Err(TlvError::BadValue),
        }
    }

    pub fn f32(self) -> Result<f32, TlvError> {
        match self {
            Value::Fixed32(v) => Ok(f32::from_bits(v)),
            _ => Err(TlvError::BadValue),
        }
    }

    pub fn bytes(self) -> Result<&'a [u8], TlvError> {
        match self {
            Value::Bytes(b) => Ok(b),
            _ => Err(TlvError::BadValue),
        }
    }

    pub fn str(self) -> Result<&'a str, TlvError> {
        std::str::from_utf8(self.bytes()?).map_err(|_| TlvError::BadUtf8)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field<'a> {
    pub no: u32,
    pub value: Value<'a>,
    /// The whole field as encoded: key and value.
    pub raw: &'a [u8],
}

/// Iterates a message's fields. After the first error it yields nothing.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn field(&mut self) -> Result<Field<'a>, TlvError> {
        let start = self.pos;
        let (key, n) = varint::read(&self.buf[self.pos..])?;
        self.pos += n;
        let no = u32::try_from(key >> 3).map_err(|_| TlvError::BadField)?;
        if no == 0 {
            return Err(TlvError::BadField);
        }
        let rest = &self.buf[self.pos..];
        let (value, n) = match (key & 7) as u8 {
            0 => {
                let (v, n) = varint::read(rest)?;
                (Value::Varint(v), n)
            }
            1 => (Value::Fixed64(u64::from_le_bytes(take(rest)?)), 8),
            2 => {
                let (len, n) = varint::read(rest)?;
                let end = usize::try_from(len).ok().and_then(|len| n.checked_add(len)).ok_or(TlvError::Truncated)?;
                (Value::Bytes(rest.get(n..end).ok_or(TlvError::Truncated)?), end)
            }
            5 => (Value::Fixed32(u32::from_le_bytes(take(rest)?)), 4),
            wire => return Err(TlvError::BadWire(wire)),
        };
        self.pos += n;
        Ok(Field { no, value, raw: &self.buf[start..self.pos] })
    }
}

fn take<const N: usize>(buf: &[u8]) -> Result<[u8; N], TlvError> {
    buf.get(..N).and_then(|b| b.try_into().ok()).ok_or(TlvError::Truncated)
}

impl<'a> Iterator for Reader<'a> {
    type Item = Result<Field<'a>, TlvError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.buf.len() {
            return None;
        }
        let field = self.field();
        if field.is_err() {
            self.pos = self.buf.len();
        }
        Some(field)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(buf: &[u8]) -> Vec<(u32, Value<'_>)> {
        Reader::new(buf).map(|f| f.map(|f| (f.no, f.value))).collect::<Result<_, _>>().unwrap()
    }

    #[test]
    fn every_wire_type_round_trips() {
        let mut inner = Writer::new();
        inner.varint(1, 5);
        let mut w = Writer::new();
        w.varint(1, 300);
        w.fixed64(2, u64::MAX);
        w.str(3, "héllo");
        w.f32(4, 1.5);
        w.message(5, &inner);
        let bytes = w.into_bytes();
        let got = fields(&bytes);
        assert_eq!(got[0], (1, Value::Varint(300)));
        assert_eq!(got[1], (2, Value::Fixed64(u64::MAX)));
        assert_eq!(got[2].1.str(), Ok("héllo"));
        assert_eq!(got[3].1.f32(), Ok(1.5));
        assert_eq!(fields(got[4].1.bytes().unwrap()), [(1, Value::Varint(5))]);
    }

    #[test]
    fn an_older_reader_skips_fields_it_does_not_know() {
        // A "newer" message: fields 1 and 3 are known, 2 and 9 are not.
        let mut w = Writer::new();
        w.varint(1, 7);
        w.bytes(2, b"from the future");
        w.fixed32(9, 1);
        w.str(3, "kept");
        let bytes = w.into_bytes();
        let (mut one, mut three) = (0, String::new());
        for field in Reader::new(&bytes) {
            let field = field.unwrap();
            match field.no {
                1 => one = field.value.u64().unwrap(),
                3 => three = field.value.str().unwrap().to_owned(),
                _ => {}
            }
        }
        assert_eq!((one, three.as_str()), (7, "kept"));
    }

    #[test]
    fn raw_is_the_whole_field_and_can_be_written_back() {
        let mut w = Writer::new();
        w.str(12, "x");
        w.varint(3, 1);
        let bytes = w.into_bytes();
        let mut copy = Writer::new();
        for field in Reader::new(&bytes) {
            copy.raw(field.unwrap().raw);
        }
        assert_eq!(copy.into_bytes(), bytes);
    }

    #[test]
    fn negative_zero_is_written_as_zero() {
        let (mut a, mut b) = (Writer::new(), Writer::new());
        a.f32(1, -0.0);
        b.f32(1, 0.0);
        assert_eq!(a.into_bytes(), b.into_bytes());
    }

    #[test]
    fn damaged_messages_are_errors() {
        let mut w = Writer::new();
        w.str(1, "abcdef");
        let bytes = w.into_bytes();
        let cut: Vec<_> = Reader::new(&bytes[..bytes.len() - 1]).collect();
        assert_eq!(cut, [Err(TlvError::Truncated)]);
        assert_eq!(Reader::new(&[(1 << 3) | 3]).next(), Some(Err(TlvError::BadWire(3))));
        assert_eq!(Reader::new(&[0]).next(), Some(Err(TlvError::BadField)));
    }
}
