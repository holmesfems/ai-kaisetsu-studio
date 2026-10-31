//! MCP の `inputSchema` を型から作る。

use schemars::generate::SchemaSettings;
use schemars::JsonSchema;
use serde_json::{Map, Value};

/// `T` の JSON Schema（MCP のツールの `inputSchema`）。
/// - `$defs`・`$ref` を使わず、すべてその場に書く
/// - `$schema`・`title`・最上位の `description`・`format` は付けない
/// - `Option` の項目は `required` に入らず、型に `null` も足さない（後ろの `drop_null`）
/// - 何でも受ける項目（`true`）は `{}` にする
pub fn input_schema<T: JsonSchema>() -> Value {
    let mut settings = SchemaSettings::draft2020_12();
    settings.inline_subschemas = true;
    settings.meta_schema = None;
    let mut v = settings.into_generator().into_root_schema_for::<T>().to_value();
    if let Some(o) = v.as_object_mut() {
        o.remove("$schema");
        o.remove("description");
    }
    clean(&mut v);
    v
}

/// 値の形を整える: `format`（int64 / double など）と型名の `title` は MCP に要らない
fn clean(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.remove("format");
            m.remove("title");
            // 「無いとき null」の既定値は、型（object など）と合わないので載せない
            if m.get("default") == Some(&Value::Null) {
                m.remove("default");
            }
            drop_null(m);
            for (k, x) in m.iter_mut() {
                match (k.as_str(), x) {
                    // properties の中身は項目名の表（title という名前の項目があってもよい）。値の 1 つ 1 つがスキーマ
                    ("properties", Value::Object(props)) => {
                        props.values_mut().for_each(clean);
                        any_to_empty(props);
                    }
                    (_, x) => clean(x),
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(clean),
        _ => {}
    }
}

/// `Option<T>` の `null` を外す: "type": ["string", "null"] は "string" に、anyOf [T, {type: null}] は T にする
fn drop_null(m: &mut Map<String, Value>) {
    if let Some(Value::Array(ts)) = m.get_mut("type") {
        ts.retain(|t| t != "null");
        if ts.len() == 1 {
            let t = ts.remove(0);
            m.insert("type".into(), t);
        }
    }
    if let Some(Value::Array(alts)) = m.get("anyOf") {
        let rest: Vec<&Value> = alts.iter().filter(|a| a.get("type") != Some(&Value::String("null".into()))).collect();
        if rest.len() == 1 && alts.len() == 2 {
            if let Value::Object(inner) = rest[0].clone() {
                m.remove("anyOf");
                for (k, v) in inner {
                    m.entry(k).or_insert(v);
                }
            }
        }
    }
}

fn any_to_empty(props: &mut Map<String, Value>) {
    for x in props.values_mut() {
        if *x == Value::Bool(true) {
            *x = Value::Object(Map::new());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::*;
    use serde_json::json;

    fn props(v: &Value) -> Vec<String> {
        let mut k: Vec<String> = v["properties"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
        k.sort();
        k
    }

    fn required(v: &Value) -> Vec<String> {
        let mut k: Vec<String> =
            v["required"].as_array().map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect()).unwrap_or_default();
        k.sort();
        k
    }

    #[test]
    fn schema_has_no_refs_or_meta() {
        let s = input_schema::<ProposeReq>();
        let text = s.to_string();
        assert!(!text.contains("$ref") && !text.contains("$defs") && !text.contains("$schema"), "{text}");
        assert!(s.get("title").is_none() && s.get("description").is_none());
        assert_eq!(s["type"], "object");
    }

    #[test]
    fn propose_item_kind_is_enum_with_description() {
        let s = input_schema::<ProposeReq>();
        assert_eq!(props(&s), ["items", "note", "title"]);
        assert_eq!(required(&s), ["items"]);
        let item = &s["properties"]["items"]["items"];
        assert_eq!(item["type"], "object");
        assert_eq!(item["properties"]["kind"]["enum"], json!(["edit", "insert", "delete", "speaker", "memo", "notes", "move", "block_add", "block_delete", "block_rename"]));
        let d = item["properties"]["kind"]["description"].as_str().unwrap();
        assert!(d.starts_with("edit=文を直す / insert=行を足す / delete=行を消す / speaker=話者だけ変える / memo="), "{d}");
        assert!(d.contains("block_add=") && d.contains("block_rename="), "{d}");
        assert_eq!(required(item), ["kind"]);
        // 何でも受ける項目は description だけ（type を付けない）
        assert!(item["properties"]["line"].get("type").is_none());
        assert_eq!(item["properties"]["line"]["description"], "対象の行（script_read の通し番号 n か、行の id）");
    }

    #[test]
    fn option_fields_are_optional_plain_types() {
        let s = input_schema::<StillReq>();
        assert_eq!(props(&s), ["cue", "scene", "solo", "subs", "t"]);
        assert!(required(&s).is_empty());
        assert_eq!(s["properties"]["t"]["type"], "number");
        assert_eq!(s["properties"]["cue"]["type"], "integer");
        assert_eq!(s["properties"]["subs"]["type"], "boolean");
        assert_eq!(s["properties"]["solo"]["description"], "このクリップだけを描く");
    }

    #[test]
    fn forced_required_and_object_types() {
        let s = input_schema::<MediaPatch>();
        assert_eq!(required(&s), ["clip_id", "patch"]);
        assert_eq!(s["properties"]["patch"]["type"], "object");
        let s = input_schema::<SeSet>();
        assert_eq!(required(&s), ["kinds"]);
        assert_eq!(s["properties"]["kinds"]["type"], "object");
        assert_eq!(s["properties"]["render"]["type"], "boolean");
    }

    #[test]
    fn export_kind_enum_matches_const() {
        let s = input_schema::<ExportReq>();
        assert_eq!(s["properties"]["kind"]["enum"], json!(EXPORT_KINDS));
        assert_eq!(required(&s), ["kind"]);
    }

    #[test]
    fn ui_focus_round_trips_without_null_keys() {
        let f: UiFocus = serde_json::from_value(json!({"t": 3.5, "scene": 57, "line": "l1"})).unwrap();
        assert_eq!(serde_json::to_value(&f).unwrap(), json!({"t": 3.5, "scene": 57, "line": "l1"}));
        let s = input_schema::<UiFocus>();
        assert_eq!(s["properties"]["scene"]["type"], "string");
        assert_eq!(s["properties"]["line"], json!({}));
    }
}
