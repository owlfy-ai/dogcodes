//! Vendor question payloads normalize into one form; native answers retain their
//! original identifiers. These functions never interpret questions as permission.
use agent_domain::{QuestionAnswer, QuestionForm};
use serde_json::{Map, Value, json};

pub fn native_form(provider: &str, params: &Value) -> Result<QuestionForm, String> {
    if provider == "acp" {
        if params["mode"] != "form" {
            return Err("Unsupported elicitation mode".into());
        }
        let schema = params["requestedSchema"].clone();
        if !schema["properties"].is_object() {
            return Err("Missing form properties".into());
        }
        return Ok(QuestionForm {
            title: params["message"]
                .as_str()
                .unwrap_or("请回答以下问题")
                .into(),
            schema,
        });
    }
    let input = if provider == "codebuddy" && params["schema"]["questions"].is_array() {
        &params["schema"]
    } else {
        params
    };
    let questions = input["questions"]
        .as_array()
        .filter(|q| !q.is_empty() && q.len() <= 12)
        .ok_or("Invalid question list")?;
    let mut props = Map::new();
    let mut required = vec![];
    for (i, q) in questions.iter().enumerate() {
        let prompt = q["question"]
            .as_str()
            .or_else(|| q["prompt"].as_str())
            .ok_or("Missing question text")?;
        let key = question_key(provider, q, i);
        if props.contains_key(&key) {
            return Err("Duplicate question identifier".into());
        }
        let options:Vec<Value>=q["options"].as_array().into_iter().flatten().map(|o|{
            let label=o.as_str().or_else(||o["label"].as_str()).unwrap_or("");
            let value=if provider=="cursor"{o["id"].as_str().unwrap_or(label)}else{o["value"].as_str().unwrap_or(label)};
            json!({"const":value,"title":label,"description":o["description"],"preview":o["preview"]})
        }).collect();
        let multiple = [
            "multiSelect",
            "multi_select",
            "allowMultiple",
            "is_multi_select",
        ]
        .iter()
        .any(|k| q[k].as_bool() == Some(true));
        let other =
            provider != "cursor" && (provider != "codex" || q["isOther"].as_bool() == Some(true));
        let mut item = json!({"type":"string","minLength":1});
        if !options.is_empty() {
            item["x-options"] = json!(options);
            if !other {
                item["oneOf"] = json!(options);
            }
            item["x-allowOther"] = json!(other);
        }
        let mut prop = if multiple {
            json!({"type":"array","items":item,"minItems":1})
        } else {
            item
        };
        prop["title"] = json!(prompt);
        prop["writeOnly"] = json!(q["isSecret"].as_bool() == Some(true));
        props.insert(key.clone(), prop);
        required.push(key);
    }
    Ok(QuestionForm {
        title: params["title"]
            .as_str()
            .or_else(|| params["prompt"].as_str())
            .unwrap_or("请回答以下问题")
            .into(),
        schema: json!({"type":"object","properties":props,"required":required}),
    })
}
fn question_key(provider: &str, q: &Value, index: usize) -> String {
    if matches!(provider, "claude" | "grok") {
        return q["question"].as_str().unwrap_or("").into();
    }
    if provider == "zcode" {
        return format!("answer_{index}");
    }
    q["id"]
        .as_str()
        .map_or_else(|| format!("q_{index}"), str::to_owned)
}
#[must_use]
pub fn native_answer(provider: &str, params: &Value, answer: &QuestionAnswer) -> Value {
    let values = answer.values.as_object().cloned().unwrap_or_default();
    let arrays: Map<String, Value> = values
        .iter()
        .map(|(k, v)| (k.clone(), if v.is_array() { v.clone() } else { json!([v]) }))
        .collect();
    match provider {
        "acp" | "zcode" => {
            if answer.cancelled {
                json!({"action":"cancel"})
            } else {
                json!({"action":"accept","content":values})
            }
        }
        "codex" => {
            let mapped: Map<String, Value> = if answer.cancelled {
                Map::new()
            } else {
                arrays
                    .into_iter()
                    .map(|(k, v)| (k, json!({"answers":v})))
                    .collect()
            };
            json!({"answers":mapped})
        }
        "claude" => {
            if answer.cancelled {
                json!({"behavior":"deny","message":"User cancelled the question. No answer was provided."})
            } else {
                let mut input = params.clone();
                input["answers"] = Value::Object(
                    values
                        .into_iter()
                        .map(|(k, v)| {
                            let v = if let Some(a) = v.as_array() {
                                json!(
                                    a.iter()
                                        .filter_map(Value::as_str)
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                )
                            } else {
                                v
                            };
                            (k, v)
                        })
                        .collect(),
                );
                json!({"behavior":"allow","updatedInput":input})
            }
        }
        "cursor" => {
            if answer.cancelled {
                json!({"outcome":{"outcome":"cancelled"}})
            } else {
                json!({"outcome":{"outcome":"answered","answers":arrays.into_iter().map(|(k,v)|json!({"questionId":k,"selectedOptionIds":v})).collect::<Vec<_>>()}})
            }
        }
        "codebuddy" => {
            if answer.cancelled {
                json!({"outcome":"cancelled"})
            } else {
                json!({"outcome":"submitted","answers":arrays})
            }
        }
        "grok" => {
            if answer.cancelled {
                json!({"outcome":"cancelled"})
            } else {
                let mut selections = Map::new();
                let mut annotations = Map::new();
                for q in params["questions"].as_array().into_iter().flatten() {
                    let key = q["question"].as_str().unwrap_or("");
                    let opts = q["options"].as_array().cloned().unwrap_or_default();
                    let mut chosen = vec![];
                    let mut notes = vec![];
                    for value in arrays[key].as_array().into_iter().flatten() {
                        if opts.iter().any(|o| o["label"] == *value) {
                            chosen.push(value.clone());
                        } else if let Some(s) = value.as_str() {
                            notes.push(s);
                        }
                    }
                    if !notes.is_empty() {
                        if chosen.is_empty() {
                            chosen.push(json!("Other"));
                        }
                        annotations.insert(key.into(), json!({"notes":notes.join("\n")}));
                    }
                    selections.insert(key.into(), json!(chosen));
                }
                json!({"outcome":"accepted","answers":selections,"annotations":annotations})
            }
        }
        _ => json!(null),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_keeps_ids_and_rejects_free_text() {
        let p = json!({"questions":[{"id":"q","prompt":"Pick","options":[{"id":"a","label":"Alpha"}],"allowMultiple":true}]});
        let f = native_form("cursor", &p).unwrap();
        let a = QuestionAnswer {
            cancelled: false,
            values: json!({"q":["a"]}),
        };
        assert!(f.validate(&a).is_ok());
        assert_eq!(
            native_answer("cursor", &p, &a)["outcome"]["answers"][0]["selectedOptionIds"],
            json!(["a"])
        );
        assert!(
            f.validate(&QuestionAnswer {
                values: json!({"q":["other"]}),
                ..a
            })
            .is_err()
        );
    }
    #[test]
    fn grok_free_text_uses_annotation_and_cancel_is_not_failure() {
        let p = json!({"questions":[{"question":"Pick","options":[{"label":"A"}]}]});
        let a = QuestionAnswer {
            cancelled: false,
            values: json!({"Pick":"B"}),
        };
        let result = native_answer("grok", &p, &a);
        assert_eq!(result["answers"]["Pick"], json!(["Other"]));
        assert_eq!(result["annotations"]["Pick"]["notes"], "B");
        assert_eq!(
            native_answer(
                "grok",
                &p,
                &QuestionAnswer {
                    cancelled: true,
                    ..a
                }
            )["outcome"],
            "cancelled"
        );
    }
}
