//! Canonical bencode (BEP 3) encoding and strict decoding.
//!
//! Signatures cover encoded bytes, so the decoder accepts only the canonical
//! form: integers without leading zeros or `-0`, dictionary keys in strictly
//! ascending order, and no trailing data. Anything else is an error, which
//! guarantees `decode(bytes)?.encode() == bytes`.

use std::collections::BTreeMap;

use crate::error::Error;

/// Deepest list/dictionary nesting accepted from untrusted input.
const MAX_DEPTH: usize = 32;

/// A bencode value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// An integer.
    Int(i64),
    /// A byte string.
    Bytes(Vec<u8>),
    /// A list.
    List(Vec<Value>),
    /// A dictionary. `BTreeMap` keeps keys in canonical (sorted) order.
    Dict(BTreeMap<Vec<u8>, Value>),
}

impl Value {
    /// Encodes the value in canonical form.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    /// Decodes exactly one canonical value spanning all of `input`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Bencode`] if `input` is malformed, non-canonical, nested
    /// deeper than 32 levels, or followed by trailing bytes.
    pub fn decode(input: &[u8]) -> Result<Value, Error> {
        let mut decoder = Decoder { input, pos: 0 };
        let value = decoder.value(0)?;
        if decoder.pos != input.len() {
            return Err(decoder.error("trailing data"));
        }
        Ok(value)
    }

    /// Returns the integer, if this is an [`Value::Int`].
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// Returns the bytes, if this is a [`Value::Bytes`].
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// Returns the items, if this is a [`Value::List`].
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(items) => Some(items),
            _ => None,
        }
    }

    /// Returns the entries, if this is a [`Value::Dict`].
    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, Value>> {
        match self {
            Value::Dict(entries) => Some(entries),
            _ => None,
        }
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Value::Int(n) => {
                out.push(b'i');
                out.extend_from_slice(n.to_string().as_bytes());
                out.push(b'e');
            }
            Value::Bytes(bytes) => encode_bytes(bytes, out),
            Value::List(items) => {
                out.push(b'l');
                for item in items {
                    item.encode_into(out);
                }
                out.push(b'e');
            }
            Value::Dict(entries) => {
                out.push(b'd');
                for (key, value) in entries {
                    encode_bytes(key, out);
                    value.encode_into(out);
                }
                out.push(b'e');
            }
        }
    }
}

fn encode_bytes(bytes: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(bytes.len().to_string().as_bytes());
    out.push(b':');
    out.extend_from_slice(bytes);
}

struct Decoder<'a> {
    input: &'a [u8],
    pos: usize,
}

impl Decoder<'_> {
    fn error(&self, reason: &'static str) -> Error {
        Error::Bencode {
            offset: self.pos,
            reason,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn value(&mut self, depth: usize) -> Result<Value, Error> {
        if depth > MAX_DEPTH {
            return Err(self.error("nesting too deep"));
        }
        match self.peek() {
            Some(b'i') => {
                self.pos += 1;
                self.integer().map(Value::Int)
            }
            Some(b'0'..=b'9') => self.bytes().map(Value::Bytes),
            Some(b'l') => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    match self.peek() {
                        None => return Err(self.error("unterminated list")),
                        Some(b'e') => break,
                        Some(_) => items.push(self.value(depth + 1)?),
                    }
                }
                self.pos += 1;
                Ok(Value::List(items))
            }
            Some(b'd') => {
                self.pos += 1;
                let mut entries: BTreeMap<Vec<u8>, Value> = BTreeMap::new();
                loop {
                    match self.peek() {
                        None => return Err(self.error("unterminated dictionary")),
                        Some(b'e') => break,
                        Some(_) => {}
                    }
                    let key_offset = self.pos;
                    let key = self.bytes()?;
                    // Keys arrive in order, so the map's last key is the previous one.
                    if entries
                        .last_key_value()
                        .is_some_and(|(prev, _)| *prev >= key)
                    {
                        return Err(Error::Bencode {
                            offset: key_offset,
                            reason: "dictionary keys not strictly ascending",
                        });
                    }
                    let value = self.value(depth + 1)?;
                    entries.insert(key, value);
                }
                self.pos += 1;
                Ok(Value::Dict(entries))
            }
            Some(_) => Err(self.error("unexpected byte")),
            None => Err(self.error("unexpected end of input")),
        }
    }

    /// Reads digits up to `terminator` and consumes the terminator.
    fn digits_until(&mut self, terminator: u8) -> Result<&[u8], Error> {
        let start = self.pos;
        let Some(len) = self.input[start..].iter().position(|&b| b == terminator) else {
            return Err(self.error("unterminated number"));
        };
        self.pos = start + len + 1;
        Ok(&self.input[start..start + len])
    }

    fn integer(&mut self) -> Result<i64, Error> {
        let start = self.pos;
        let digits = self.digits_until(b'e')?;
        let magnitude = digits.strip_prefix(b"-").unwrap_or(digits);
        if !is_canonical_unsigned(magnitude) || digits == b"-0" {
            return Err(Error::Bencode {
                offset: start,
                reason: "non-canonical integer",
            });
        }
        parse_ascii(digits).ok_or(Error::Bencode {
            offset: start,
            reason: "integer out of range",
        })
    }

    fn bytes(&mut self) -> Result<Vec<u8>, Error> {
        let start = self.pos;
        let digits = self.digits_until(b':')?;
        if !is_canonical_unsigned(digits) {
            return Err(Error::Bencode {
                offset: start,
                reason: "non-canonical string length",
            });
        }
        let len: usize = parse_ascii(digits).ok_or(Error::Bencode {
            offset: start,
            reason: "string length out of range",
        })?;
        let end = self
            .pos
            .checked_add(len)
            .filter(|&end| end <= self.input.len())
            .ok_or(Error::Bencode {
                offset: start,
                reason: "string runs past end of input",
            })?;
        let bytes = self.input[self.pos..end].to_vec();
        self.pos = end;
        Ok(bytes)
    }
}

/// One or more ASCII digits, with no leading zero unless the number is `0`.
fn is_canonical_unsigned(digits: &[u8]) -> bool {
    !digits.is_empty()
        && digits.iter().all(u8::is_ascii_digit)
        && (digits.len() == 1 || digits[0] != b'0')
}

fn parse_ascii<T: std::str::FromStr>(digits: &[u8]) -> Option<T> {
    std::str::from_utf8(digits).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn dict(entries: &[(&str, Value)]) -> Value {
        Value::Dict(
            entries
                .iter()
                .map(|(k, v)| (k.as_bytes().to_vec(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn encodes_bep3_examples() {
        assert_eq!(Value::Int(3).encode(), b"i3e");
        assert_eq!(Value::Int(-3).encode(), b"i-3e");
        assert_eq!(Value::Bytes(b"spam".to_vec()).encode(), b"4:spam");
        let list = Value::List(vec![
            Value::Bytes(b"spam".to_vec()),
            Value::Bytes(b"eggs".to_vec()),
        ]);
        assert_eq!(list.encode(), b"l4:spam4:eggse");
        let d = dict(&[
            ("spam", Value::Bytes(b"eggs".to_vec())),
            ("cow", Value::Bytes(b"moo".to_vec())),
        ]);
        assert_eq!(
            d.encode(),
            b"d3:cow3:moo4:spam4:eggse",
            "keys must be emitted sorted"
        );
    }

    #[test]
    fn rejects_non_canonical_input() {
        let cases: &[&[u8]] = &[
            b"i03e",                // leading zero
            b"i-0e",                // negative zero
            b"ie",                  // empty integer
            b"i+3e",                // explicit plus
            b"03:abc",              // leading zero in length
            b"d4:spam1:a3:cow1:be", // unsorted keys
            b"d3:cow1:a3:cow1:be",  // duplicate keys
            b"i1ei2e",              // trailing data
            b"l",                   // unterminated list
            b"5:abc",               // length past end
            b"i99999999999999999999e",
            b"x",
            b"",
        ];
        for case in cases {
            assert!(
                Value::decode(case).is_err(),
                "{:?} is not canonical bencode and must be rejected",
                String::from_utf8_lossy(case)
            );
        }
    }

    #[test]
    fn rejects_deep_nesting() {
        let depth = MAX_DEPTH + 2;
        let mut input = vec![b'l'; depth];
        input.extend(std::iter::repeat_n(b'e', depth));
        assert!(
            Value::decode(&input).is_err(),
            "nesting beyond MAX_DEPTH must be rejected"
        );
    }

    fn arb_value() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            any::<i64>().prop_map(Value::Int),
            prop::collection::vec(any::<u8>(), 0..16).prop_map(Value::Bytes),
        ];
        leaf.prop_recursive(4, 64, 8, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..8).prop_map(Value::List),
                prop::collection::btree_map(prop::collection::vec(any::<u8>(), 0..8), inner, 0..8)
                    .prop_map(Value::Dict),
            ]
        })
    }

    proptest! {
        #[test]
        fn round_trips(value in arb_value()) {
            let encoded = value.encode();
            prop_assert_eq!(Value::decode(&encoded), Ok(value));
        }

        #[test]
        fn decode_is_total_and_canonical(bytes in prop::collection::vec(any::<u8>(), 0..64)) {
            // Must never panic, and anything accepted must re-encode identically.
            if let Ok(value) = Value::decode(&bytes) {
                prop_assert_eq!(value.encode(), bytes);
            }
        }
    }
}
