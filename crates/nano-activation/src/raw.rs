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
    parse_frame(raw)
}

pub(crate) fn locate_control(frame: &Value) -> Result<&Value, ActivationError> {
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

    // The exact carrier must occur in canonical form inside the whole raw frame. This
    // retains ordering/escaping evidence without normalizing or reparsing transport bytes.
    let canonical = serde_jcs::to_vec(carrier)
        .map_err(|_| ActivationError::new(RejectReason::NoncanonicalPayload))?;
    if !raw
        .windows(canonical.len())
        .any(|window| window == canonical)
    {
        return Err(ActivationError::new(RejectReason::NoncanonicalPayload));
    }
    Ok(carrier)
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
