//! 严格 JSON：重复键、非有限数值一律拒绝（serde_json 默认对重复键取后一个）；规范化序列化用于指纹。

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use std::fmt;

struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StrictVisitor).map(Strict)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("JSON 值")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        Number::from_f64(v).map(Value::Number).ok_or_else(|| E::custom("数值必须有限"))
    }
    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut out = Vec::new();
        while let Some(Strict(v)) = seq.next_element()? {
            out.push(v);
        }
        Ok(Value::Array(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if out.contains_key(&key) {
                return Err(de::Error::custom(format!("重复字段：{key}")));
            }
            let Strict(v) = map.next_value()?;
            out.insert(key, v);
        }
        Ok(Value::Object(out))
    }
}

/// 解析一段 JSON 文本；重复键、尾随内容、非有限数值都报错。
pub fn parse(text: &str) -> Result<Value, String> {
    let mut de = serde_json::Deserializer::from_str(text);
    let Strict(value) = Strict::deserialize(&mut de).map_err(|e| e.to_string())?;
    de.end().map_err(|e| e.to_string())?;
    Ok(value)
}

/// 规范化文本：键按字典序（serde_json 的 Map 默认是有序的），无多余空白。
pub fn canonical(value: &Value) -> String {
    serde_json::to_string(value).expect("JSON 值总能序列化")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// 用小写十六进制编码字节，每字节占两位。
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_duplicate_keys_at_any_depth() {
        assert!(parse(r#"{"a":1,"a":2}"#).is_err());
        assert!(parse(r#"{"a":{"b":[{"c":1,"c":1}]}}"#).is_err());
        assert!(parse(r#"{"a":1,"b":{"a":2}}"#).is_ok());
    }

    #[test]
    fn rejects_non_finite_and_trailing_content() {
        assert!(parse("NaN").is_err());
        assert!(parse("1e999").is_err());
        assert!(parse(r#"{"a":1} {"b":2}"#).is_err());
    }

    #[test]
    fn canonical_form_sorts_keys() {
        let a = parse(r#"{"b":1,"a":{"d":0.5,"c":"x"}}"#).unwrap();
        let b = parse(r#"{ "a": {"c":"x","d":0.5}, "b": 1 }"#).unwrap();
        assert_eq!(canonical(&a), r#"{"a":{"c":"x","d":0.5},"b":1}"#);
        assert_eq!(canonical(&a), canonical(&b));
        // 以前的 Python canonical() 正好给出这些字节（#9）。
        let c = parse("{\"b\":[1,true,null,\"é\\n\"],\"a\":{\"y\":2.5e-05}}").unwrap();
        assert_eq!(canonical(&c), "{\"a\":{\"y\":0.000025},\"b\":[1,true,null,\"é\\n\"]}");
    }
}

#[cfg(test)]
mod float_layout {
    /// tests/reference/provenance/export.py（#9 已删除） 的 canonical 要逐字符复现这套排版（原 tests/test_terms_artists.py 的 CanonicalTest 用同一组字面值，#9 已删除）。
    #[test]
    fn serde_json_lays_out_floats_like_this() {
        let values =
            [1e-7, 5.72e-5, 1e-5, 0.1, 123.0, 1e15, 1e16, 1e21, 1.5e22, -0.0, 2.099447491238697e-9, 0.97, -3.25];
        let text = serde_json::to_string(&values.to_vec()).unwrap();
        assert_eq!(
            text,
            "[1e-7,0.0000572,0.00001,0.1,123.0,1000000000000000.0,1e+16,1e+21,1.5e+22,-0.0,2.099447491238697e-9,0.97,-3.25]"
        );
    }
}
