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
            return Err("回答内容过长".into());
        }
        let values = answer.values.as_object().ok_or("回答必须为表单对象")?;
        let props = self.schema["properties"]
            .as_object()
            .ok_or("不支持此问题格式")?;
        for key in values.keys() {
            if !props.contains_key(key) {
                return Err("回答包含未知字段".into());
            }
        }
        for key in self.schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !values.contains_key(key) {
                return Err("请回答所有必填问题".into());
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
        return Err("请选择有效选项".into());
    }
    if let Some(options) = schema["oneOf"]
        .as_array()
        .or_else(|| schema["anyOf"].as_array())
        && !options.iter().any(|o| o.get("const") == Some(value))
    {
        return Err("请选择有效选项".into());
    }
    match schema["type"].as_str().unwrap_or("string") {
        "string" => {
            let s = value.as_str().ok_or("请输入文字")?;
            let len = s.chars().count() as u64;
            if schema["minLength"].as_u64().is_some_and(|n| len < n)
                || schema["maxLength"].as_u64().is_some_and(|n| len > n)
            {
                return Err("文字长度不符合要求".into());
            }
            if let Some(pattern) = schema["pattern"].as_str() {
                let re = regex::Regex::new(pattern).map_err(|_| "不支持此校验格式")?;
                if !re.is_match(s) {
                    return Err("输入格式不符合要求".into());
                }
            }
        }
        "boolean" => {
            if !value.is_boolean() {
                return Err("请选择是或否".into());
            }
        }
        "integer" | "number" => {
            let n = value
                .as_f64()
                .filter(|n| n.is_finite())
                .ok_or("请输入数字")?;
            if schema["type"] == "integer" && n.fract() != 0.0 {
                return Err("请输入整数".into());
            }
            if schema["minimum"].as_f64().is_some_and(|v| n < v)
                || schema["maximum"].as_f64().is_some_and(|v| n > v)
            {
                return Err("数字超出允许范围".into());
            }
        }
        "array" => {
            let values = value.as_array().ok_or("请选择选项")?;
            if schema["minItems"]
                .as_u64()
                .is_some_and(|n| (values.len() as u64) < n)
                || schema["maxItems"]
                    .as_u64()
                    .is_some_and(|n| (values.len() as u64) > n)
            {
                return Err("选项数量不符合要求".into());
            }
            for (i, v) in values.iter().enumerate() {
                if values[..i].contains(v) {
                    return Err("选项不能重复".into());
                }
                validate_value(&schema["items"], v)?;
            }
        }
        _ => return Err("暂不支持此问题类型".into()),
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
