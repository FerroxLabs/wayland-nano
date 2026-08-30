use serde::de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::{collections::HashSet, fmt};

use crate::{ActivationError, RejectReason};

pub(crate) const MAX_FRAME_BYTES: usize = 32 * 1024;
const MAX_DEPTH: usize = 8;
const MAX_PROPERTIES: usize = 256;
const MAX_ARRAY_ITEMS: usize = 64;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

pub(crate) fn parse_frame(raw: &[u8]) -> Result<Value, ActivationError> {
    if raw.len() > MAX_FRAME_BYTES {
        return Err(ActivationError::new(RejectReason::CarrierOversized));
    }
    let text =
        std::str::from_utf8(raw).map_err(|_| ActivationError::new(RejectReason::MalformedJson))?;
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let mut properties = 0usize;
    let value = StrictSeed {
        depth: 0,
        properties: &mut properties,
    }
    .deserialize(&mut deserializer)
    .map_err(map_parse_error)?;
    deserializer.end().map_err(map_parse_error)?;
    Ok(value)
}

pub(crate) fn parse_transport_frame(raw: &[u8]) -> Result<Value, ActivationError> {
    if raw.len() > MAX_FRAME_BYTES {
        return Err(ActivationError::new(RejectReason::CarrierOversized));
    }
    if raw.first() != Some(&b'{') || raw.last() != Some(&b'}') {
        return Err(ActivationError::new(RejectReason::NoncanonicalPayload));
    }
    parse_frame(raw)
}

pub(crate) fn locate_control<'a>(
    raw: &'a [u8],
    frame: &'a Value,
) -> Result<&'a Value, ActivationError> {
    let control = frame
        .as_object()
        .and_then(|object| object.get("params"))
        .and_then(Value::as_object)
        .and_then(|object| object.get("_meta"))
        .and_then(Value::as_object)
        .and_then(|object| object.get("waylandNanoControl"))
        .ok_or_else(|| ActivationError::new(RejectReason::CarrierMissing))?;
    if !control.is_object() {
        return Err(ActivationError::new(RejectReason::MalformedJson));
    }
    require_exact_canonical_slice(raw, &["params", "_meta", "waylandNanoControl"], control)?;
    Ok(control)
}

pub(crate) fn locate_activation<'a>(
    raw: &'a [u8],
    frame: &'a Value,
) -> Result<&'a Value, ActivationError> {
    let meta = frame
        .as_object()
        .and_then(|object| object.get("params"))
        .and_then(Value::as_object)
        .and_then(|object| object.get("_meta"))
        .and_then(Value::as_object)
        .ok_or_else(|| ActivationError::new(RejectReason::CarrierMissing))?;
    let carrier = meta
        .get("waylandNanoActivation")
        .ok_or_else(|| ActivationError::new(RejectReason::CarrierMissing))?;
    if !carrier.is_object() {
        return Err(ActivationError::new(RejectReason::MalformedJson));
    }

    require_exact_canonical_slice(raw, &["params", "_meta", "waylandNanoActivation"], carrier)?;
    Ok(carrier)
}

fn require_exact_canonical_slice(
    raw: &[u8],
    path: &[&str],
    value: &Value,
) -> Result<(), ActivationError> {
    let (start, end) = locate_path_span(raw, path)
        .ok_or_else(|| ActivationError::new(RejectReason::NoncanonicalPayload))?;
    let canonical = serde_jcs::to_vec(value)
        .map_err(|_| ActivationError::new(RejectReason::NoncanonicalPayload))?;
    if raw.get(start..end) != Some(canonical.as_slice()) {
        return Err(ActivationError::new(RejectReason::NoncanonicalPayload));
    }
    Ok(())
}

/// Locate a value by its structural object path without normalizing the raw
/// bytes. The strict parser has already rejected duplicate keys; this second
/// pass exists solely to compare the *located* carrier rather than accepting
/// an unrelated canonical byte sequence elsewhere in the frame.
fn locate_path_span(raw: &[u8], path: &[&str]) -> Option<(usize, usize)> {
    fn ws(raw: &[u8], mut at: usize) -> usize {
        while raw.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        at
    }
    fn string_end(raw: &[u8], mut at: usize) -> Option<usize> {
        if raw.get(at)? != &b'"' {
            return None;
        }
        at += 1;
        while at < raw.len() {
            match raw[at] {
                b'\\' => at = at.checked_add(2)?,
                b'"' => return Some(at + 1),
                _ => at += 1,
            }
        }
        None
    }
    fn value_end(raw: &[u8], at: usize) -> Option<usize> {
        let at = ws(raw, at);
        match *raw.get(at)? {
            b'"' => string_end(raw, at),
            b'{' | b'[' => {
                let open = raw[at];
                let close = if open == b'{' { b'}' } else { b']' };
                let mut depth = 1usize;
                let mut pos = at + 1;
                while pos < raw.len() {
                    match raw[pos] {
                        b'"' => pos = string_end(raw, pos)?,
                        byte if byte == open => {
                            depth += 1;
                            pos += 1;
                        }
                        byte if byte == close => {
                            depth -= 1;
                            pos += 1;
                            if depth == 0 {
                                return Some(pos);
                            }
                        }
                        // Nested objects and arrays of the other type are
                        // skipped as complete values so their delimiters do
                        // not affect this container's depth.
                        b'{' | b'[' => pos = value_end(raw, pos)?,
                        _ => pos += 1,
                    }
                }
                None
            }
            _ => {
                let mut pos = at;
                while pos < raw.len() && !matches!(raw[pos], b',' | b'}' | b']') {
                    pos += 1;
                }
                Some(ws_back(raw, pos))
            }
        }
    }
    fn ws_back(raw: &[u8], mut at: usize) -> usize {
        while at > 0 && raw[at - 1].is_ascii_whitespace() {
            at -= 1;
        }
        at
    }
    fn descend(raw: &[u8], at: usize, path: &[&str]) -> Option<(usize, usize)> {
        let mut pos = ws(raw, at);
        if raw.get(pos)? != &b'{' {
            return None;
        }
        pos += 1;
        loop {
            pos = ws(raw, pos);
            if raw.get(pos)? == &b'}' {
                return None;
            }
            let key_start = pos;
            let key_end = string_end(raw, pos)?;
            let key: String = serde_json::from_slice(&raw[key_start..key_end]).ok()?;
            pos = ws(raw, key_end);
            if raw.get(pos)? != &b':' {
                return None;
            }
            let value_start = ws(raw, pos + 1);
            let end = value_end(raw, value_start)?;
            if key == path[0] {
                return if path.len() == 1 {
                    Some((value_start, end))
                } else {
                    descend(raw, value_start, &path[1..])
                };
            }
            pos = ws(raw, end);
            match raw.get(pos)? {
                b',' => pos += 1,
                b'}' => return None,
                _ => return None,
            }
        }
    }
    (!path.is_empty()).then(|| descend(raw, 0, path)).flatten()
}

fn map_parse_error(error: serde_json::Error) -> ActivationError {
    let reason = if error.to_string().starts_with("duplicate key ") {
        RejectReason::DuplicateKey
    } else {
        RejectReason::MalformedJson
    };
    ActivationError::new(reason)
}

struct StrictSeed<'a> {
    depth: usize,
    properties: &'a mut usize,
}

impl<'de> DeserializeSeed<'de> for StrictSeed<'_> {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if self.depth > MAX_DEPTH {
            return Err(D::Error::custom("maximum JSON depth exceeded"));
        }
        deserializer.deserialize_any(StrictVisitor {
            depth: self.depth,
            properties: self.properties,
        })
    }
}

struct StrictVisitor<'a> {
    depth: usize,
    properties: &'a mut usize,
}

impl<'de> Visitor<'de> for StrictVisitor<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded I-JSON")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_str<E>(self, value: &str) -> Result<Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Value, E>
    where
        E: serde::de::Error,
    {
        if value > MAX_SAFE_INTEGER {
            return Err(E::custom("integer exceeds I-JSON safe range"));
        }
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Value, E>
    where
        E: serde::de::Error,
    {
        if value < 0 || value as u64 > MAX_SAFE_INTEGER {
            return Err(E::custom("integer outside unsigned I-JSON safe range"));
        }
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Value, E>
    where
        E: serde::de::Error,
    {
        Err(E::custom("fractional JSON numbers are not permitted"))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(StrictSeed {
            depth: self.depth + 1,
            properties: self.properties,
        })? {
            if values.len() == MAX_ARRAY_ITEMS {
                return Err(A::Error::custom("maximum array length exceeded"));
            }
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        let mut keys = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(A::Error::custom(format!("duplicate key {key}")));
            }
            *self.properties += 1;
            if *self.properties > MAX_PROPERTIES {
                return Err(A::Error::custom("maximum property count exceeded"));
            }
            let value = map.next_value_seed(StrictSeed {
                depth: self.depth + 1,
                properties: self.properties,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}
