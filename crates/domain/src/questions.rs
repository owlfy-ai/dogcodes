//! Structured input is independent of permission decisions. Answers are validated
//! against the original form before they can be delivered to a live native request.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuestionForm {
    pub title: String,
    pub schema: Value,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuestionAnswer {
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub values: Value,
}
impl QuestionForm {
    pub fn validate(&self, answer: &QuestionAnswer) -> Result<(), String> {
        if answer.cancelled {
            return Ok(());
        }
        if answer.values.to_string().len() > 128_000 {
            return Err("backendText.m3f54ab3e".into());
        }
        let values = answer.values.as_object().ok_or("backendText.m472145a9")?;
        let props = self.schema["properties"]
            .as_object()
            .ok_or("backendText.m195411bd")?;
        for key in values.keys() {
            if !props.contains_key(key) {
                return Err("backendText.m9427b5f3".into());
            }
        }
        for key in self.schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !values.contains_key(key) {
                return Err("backendText.m636e4f60".into());
            }
        }
        for (key, value) in values {
            validate_value(&props[key], value)?;
        }
        Ok(())
    }
}
fn validate_value(schema: &Value, value: &Value) -> Result<(), String> {
    if let Some(options) = schema["enum"].as_array()
        && !options.contains(value)
    {
        return Err("backendText.mefd04661".into());
    }
    if let Some(options) = schema["oneOf"]
        .as_array()
        .or_else(|| schema["anyOf"].as_array())
        && !options.iter().any(|o| o.get("const") == Some(value))
    {
        return Err("backendText.mefd04661".into());
    }
    match schema["type"].as_str().unwrap_or("string") {
        "string" => {
            let s = value.as_str().ok_or("backendText.m91e09dc3")?;
            let len = s.chars().count() as u64;
            if schema["minLength"].as_u64().is_some_and(|n| len < n)
                || schema["maxLength"].as_u64().is_some_and(|n| len > n)
            {
                return Err("backendText.m99111038".into());
            }
            if let Some(pattern) = schema["pattern"].as_str() {
                let re = regex::Regex::new(pattern).map_err(|_| "backendText.m536ae399")?;
                if !re.is_match(s) {
                    return Err("backendText.m51e64d01".into());
                }
            }
        }
        "boolean" => {
            if !value.is_boolean() {
                return Err("backendText.mff99b341".into());
            }
        }
        "integer" | "number" => {
            let n = value
                .as_f64()
                .filter(|n| n.is_finite())
                .ok_or("backendText.mfa388b72")?;
            if schema["type"] == "integer" && n.fract() != 0.0 {
                return Err("backendText.m9c751154".into());
            }
            if schema["minimum"].as_f64().is_some_and(|v| n < v)
                || schema["maximum"].as_f64().is_some_and(|v| n > v)
            {
                return Err("backendText.m01cb4b8d".into());
            }
        }
        "array" => {
            let values = value.as_array().ok_or("backendText.mc2089899")?;
            if schema["minItems"]
                .as_u64()
                .is_some_and(|n| (values.len() as u64) < n)
                || schema["maxItems"]
                    .as_u64()
                    .is_some_and(|n| (values.len() as u64) > n)
            {
                return Err("backendText.m9eadaf9b".into());
            }
            for (i, v) in values.iter().enumerate() {
                if values[..i].contains(v) {
                    return Err("backendText.m69b885d7".into());
                }
                validate_value(&schema["items"], v)?;
            }
        }
        _ => return Err("backendText.md5d6d9e8".into()),
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn answers_are_typed_and_cannot_invent_options() {
        let form = QuestionForm {
            title: "test".into(),
            schema: json!({"properties":{"color":{"type":"string","enum":["red","blue"]},"count":{"type":"integer","minimum":1}},"required":["color"]}),
        };
        let check = |values| {
            form.validate(&QuestionAnswer {
                cancelled: false,
                values,
            })
        };
        assert!(check(json!({"color":"red","count":2})).is_ok());
        for bad in [
            json!({}),
            json!({"color":"green"}),
            json!({"color":"red","count":1.5}),
            json!({"color":"red","extra":"x"}),
        ] {
            assert!(check(bad).is_err());
        }
        assert!(
            form.validate(&QuestionAnswer {
                cancelled: true,
                values: Value::Null
            })
            .is_ok()
        );
    }
}
