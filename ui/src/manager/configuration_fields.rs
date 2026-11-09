//! Field parsing and stable order are shared by rendering and submit validation.
use serde_json::{Value, json};
pub fn ordered(schema: &Value) -> Vec<(&String, &Value)> {
    let mut fields = schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    fields.sort_by(|(ak, av), (bk, bv)| {
        av["order"]
            .as_i64()
            .unwrap_or(i64::MAX)
            .cmp(&bv["order"].as_i64().unwrap_or(i64::MAX))
            .then_with(|| ak.cmp(bk))
    });
    fields
}
pub fn parse(schema: &Value, text: &str) -> Result<Value, String> {
    let value = if schema["type"] == "string" {
        json!(text)
    } else {
        serde_json::from_str(text).map_err(|_| crate::i18n::tr("请输入有效的数字或 JSON 值"))?
    };
    validate(schema, &value)?;
    Ok(value)
}
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    peripheral_repl::extras::validate(schema, value).map_err(|e| {
        if e.message.contains("number out of range") {
            format!("{}: {}", crate::i18n::tr("超出允许范围"), range(schema))
        } else if e.message.contains("expected integer") {
            crate::i18n::tr("请输入整数")
        } else {
            e.message
        }
    })
}
pub fn range(schema: &Value) -> String {
    let low = schema
        .get("minimum")
        .map(ToString::to_string)
        .unwrap_or_else(|| "−∞".into());
    let high = schema
        .get("maximum")
        .map(ToString::to_string)
        .unwrap_or_else(|| "+∞".into());
    format!("{low} – {high}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_order_and_numeric_constraints() {
        for properties in [
            r#"{"z":{"order":1},"b":{},"a":{},"c":{"order":1},"first":{"order":-1}}"#,
            r#"{"a":{},"first":{"order":-1},"c":{"order":1},"b":{},"z":{"order":1}}"#,
        ] {
            let schema = json!({"properties":serde_json::from_str::<Value>(properties).unwrap()});
            assert_eq!(
                ordered(&schema)
                    .iter()
                    .map(|(k, _)| k.as_str())
                    .collect::<Vec<_>>(),
                ["first", "c", "z", "a", "b"]
            );
        }
        let schema = json!({"type":"integer","minimum":15000,"maximum":300000});
        assert!(parse(&schema, "3000").unwrap_err().contains("15000"));
        assert!(parse(&schema, "15000.5").is_err());
        assert_eq!(parse(&schema, "15000").unwrap(), json!(15000));
        assert!(parse(&schema, "300001").is_err());
    }
}
