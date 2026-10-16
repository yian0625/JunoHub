//! Stateful Responses -> Chat Completions projection. Only public summaries
//! are exposed; encrypted/private reasoning is never converted into text.
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

use crate::sub2_protocol::{
    chat_chunk, response_error_message, responses_chat_usage, responses_finish_reason,
};

#[derive(Default)]
struct Tool {
    id: String,
    name: String,
    args: String,
    started: bool,
}

/// Tracks each content part independently so terminal snapshots only emit the
/// suffix missing from that part's deltas, even across multiple messages.
#[derive(Default)]
struct TextParts {
    parts: HashMap<(i64, i64), String>,
}

impl TextParts {
    fn update(
        &mut self,
        key: (i64, i64),
        text: &str,
        delta: bool,
    ) -> Result<Option<String>, &'static str> {
        if text.is_empty() {
            return Ok(None);
        }
        let previous = self.parts.entry(key).or_default();
        let addition = if delta {
            text
        } else {
            text.strip_prefix(previous.as_str())
                .ok_or("upstream final content differs from streamed content")?
        };
        if addition.is_empty() {
            return Ok(None);
        }
        let addition = addition.to_owned();
        previous.push_str(&addition);
        Ok(Some(addition))
    }
}

#[derive(Default)]
pub(crate) struct ChatBridge {
    tools: BTreeMap<i64, Tool>,
    summaries: TextParts,
    text: TextParts,
    refusal: TextParts,
    item_indexes: HashMap<String, i64>,
    pub(crate) completed: bool,
}

fn args_text(item: &Value) -> Option<String> {
    let value = item.get("arguments")?;
    if let Some(s) = value.as_str() {
        Some(s.into())
    } else if value.is_object() {
        Some(value.to_string())
    } else {
        None
    }
}

impl ChatBridge {
    fn item_index(&mut self, index: i64, id: Option<&str>) -> i64 {
        match id.filter(|id| !id.is_empty()) {
            Some(id) => *self.item_indexes.entry(id.to_owned()).or_insert(index),
            None => index,
        }
    }

    fn full_tool(&mut self, index: i64, item: &Value) -> Result<Vec<Value>, String> {
        let call_id = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let index = self
            .tools
            .iter()
            .find_map(|(index, tool)| (!call_id.is_empty() && tool.id == call_id).then_some(*index))
            .unwrap_or(index);
        let tool = self.tools.entry(index).or_default();
        let mut out = Vec::new();
        if !call_id.is_empty() {
            tool.id = call_id.into();
        }
        if let Some(name) = item.get("name").and_then(Value::as_str) {
            tool.name = name.into();
        }
        if !tool.started && !tool.name.is_empty() {
            tool.started = true;
            out.push(json!({"tool_calls":[{"index":index,"id":tool.id,"type":"function","function":{"name":tool.name,"arguments":""}}]}));
        }
        if let Some(full) = args_text(item) {
            let suffix = full
                .strip_prefix(&tool.args)
                .ok_or("upstream final tool arguments differ from streamed arguments")?
                .to_owned();
            if !suffix.is_empty() {
                tool.args = full;
                out.push(json!({"tool_calls":[{"index":index,"function":{"arguments":suffix}}]}));
            }
        }
        Ok(out)
    }

    fn content(
        &mut self,
        index: i64,
        part: i64,
        field: &str,
        text: &str,
        delta: bool,
    ) -> Result<Option<Value>, String> {
        let parts = if field == "refusal" {
            &mut self.refusal
        } else {
            &mut self.text
        };
        let Some(addition) = parts.update((index, part), text, delta)? else {
            return Ok(None);
        };
        let mut value = serde_json::Map::new();
        value.insert(field.into(), Value::String(addition));
        Ok(Some(Value::Object(value)))
    }

    fn content_part(
        &mut self,
        index: i64,
        part_index: i64,
        part: &Value,
    ) -> Result<Option<Value>, String> {
        let (field, text) = match part["type"].as_str() {
            Some("output_text") => ("content", part["text"].as_str()),
            Some("refusal") => ("refusal", part["refusal"].as_str()),
            _ => return Ok(None),
        };
        self.content(index, part_index, field, text.unwrap_or_default(), false)
    }

    fn summary(
        &mut self,
        index: i64,
        part: i64,
        text: &str,
        delta: bool,
    ) -> Result<Option<Value>, String> {
        let key = (index, part);
        let new_part = !self.summaries.parts.contains_key(&key) && !self.summaries.parts.is_empty();
        let Some(addition) = self.summaries.update(key, text, delta)? else {
            return Ok(None);
        };
        Ok(Some(
            json!({"reasoning_content": if new_part { format!("\n\n{addition}") } else { addition }}),
        ))
    }

    fn full_item(&mut self, index: i64, item: &Value) -> Result<Vec<Value>, String> {
        let index = self.item_index(index, item.get("id").and_then(Value::as_str));
        let mut out = Vec::new();
        match item["type"].as_str() {
            Some("function_call") => out.extend(self.full_tool(index, item)?),
            Some("message") => {
                for (part_index, part) in item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    if let Some(delta) = self.content_part(index, part_index as i64, part)? {
                        out.push(delta);
                    }
                }
            }
            Some("reasoning") => {
                for (part_index, part) in item
                    .get("summary")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    if part["type"] != "summary_text" {
                        continue;
                    }
                    if let Some(delta) = self.summary(
                        index,
                        part_index as i64,
                        part["text"].as_str().unwrap_or_default(),
                        false,
                    )? {
                        out.push(delta);
                    }
                }
            }
            _ => {}
        }
        Ok(out)
    }

    pub(crate) fn convert(
        &mut self,
        event: &Value,
        id: &str,
        model: &str,
        created: i64,
    ) -> Result<Vec<Value>, String> {
        if self.completed {
            return Ok(Vec::new());
        }
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let index = event
            .get("output_index")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let index = self.item_index(index, event.get("item_id").and_then(Value::as_str));
        let part_index = event
            .get("content_index")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let mut deltas = Vec::new();
        match kind {
            "response.created" => deltas.push(json!({"role":"assistant"})),
            "response.output_text.delta"
            | "response.output_text.done"
            | "response.refusal.delta"
            | "response.refusal.done" => {
                let delta = kind.ends_with(".delta");
                let refusal = kind.starts_with("response.refusal.");
                let field = if refusal { "refusal" } else { "content" };
                let source = if delta {
                    "delta"
                } else if refusal {
                    "refusal"
                } else {
                    "text"
                };
                if let Some(value) = self.content(
                    index,
                    part_index,
                    field,
                    event[source].as_str().unwrap_or_default(),
                    delta,
                )? {
                    deltas.push(value);
                }
            }
            "response.content_part.done" => {
                if let Some(value) = self.content_part(index, part_index, &event["part"])? {
                    deltas.push(value);
                }
            }
            "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_summary_part.done" => {
                let part = event
                    .get("summary_index")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let delta = kind.ends_with(".delta");
                let text = if delta {
                    event.get("delta")
                } else if kind == "response.reasoning_summary_part.done" {
                    event.pointer("/part/text")
                } else {
                    event.get("text")
                };
                if let Some(value) = self.summary(
                    index,
                    part,
                    text.and_then(Value::as_str).unwrap_or_default(),
                    delta,
                )? {
                    deltas.push(value);
                }
            }
            "response.output_item.added" | "response.output_item.done" => {
                deltas.extend(self.full_item(index, &event["item"])?)
            }
            "response.function_call_arguments.delta" => {
                let delta = event
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.tools.entry(index).or_default().args.push_str(delta);
                deltas.push(json!({"tool_calls":[{"index":index,"function":{"arguments":delta}}]}));
            }
            "response.function_call_arguments.done" => deltas.extend(self.full_tool(index, event)?),
            "response.failed" | "error" => return Err(response_error_message(event)),
            "response.completed" | "response.incomplete" => {
                let response = event
                    .get("response")
                    .filter(|response| response.is_object())
                    .ok_or("upstream terminal event is missing a response object")?;
                if response
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|status| !matches!(status, "completed" | "incomplete"))
                {
                    return Err(response_error_message(event));
                }
                for (item_index, item) in response
                    .get("output")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    deltas.extend(self.full_item(item_index as i64, item)?);
                }
                let mut out = deltas
                    .into_iter()
                    .map(|delta| chat_chunk(id, model, created, delta, Value::Null, Value::Null))
                    .collect::<Vec<_>>();
                let finish_reason = responses_finish_reason(
                    response,
                    Some(kind),
                    !self.tools.is_empty(),
                    !self.refusal.parts.is_empty(),
                );
                self.completed = true;
                out.push(chat_chunk(
                    id,
                    model,
                    created,
                    json!({}),
                    json!(finish_reason),
                    responses_chat_usage(response),
                ));
                return Ok(out);
            }
            _ => {}
        }
        Ok(deltas
            .into_iter()
            .map(|delta| chat_chunk(id, model, created, delta, Value::Null, Value::Null))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn delta_text(frames: &[Value], field: &str) -> String {
        frames
            .iter()
            .filter_map(|frame| frame["choices"][0]["delta"][field].as_str())
            .collect()
    }

    #[test]
    fn final_text_events_restore_missing_deltas_without_repeating_text() {
        let item = json!({"id":"msg1","type":"message","content":[{"type":"output_text","text":"Hello world"}]});
        let frames = project(vec![
            json!({"type":"response.output_text.delta","item_id":"msg1","output_index":2,"content_index":0,"delta":"Hello "}),
            json!({"type":"response.output_text.done","item_id":"msg1","output_index":2,"content_index":0,"text":"Hello world"}),
            json!({"type":"response.output_item.done","output_index":2,"item":item}),
            json!({"type":"response.completed","response":{"output":[]}}),
        ]);
        assert_eq!(delta_text(&frames, "content"), "Hello world");
    }

    #[test]
    fn message_done_only_restores_multiple_parts_and_messages() {
        let frames = project(vec![
            json!({"type":"response.output_item.done","output_index":1,"item":{"id":"msg1","type":"message","content":[{"type":"output_text","text":"A"},{"type":"output_text","text":"B"}]}}),
            json!({"type":"response.output_item.done","output_index":3,"item":{"id":"msg2","type":"message","content":[{"type":"output_text","text":"C"}]}}),
            json!({"type":"response.completed","response":{"output":[]}}),
        ]);
        assert_eq!(delta_text(&frames, "content"), "ABC");
    }

    #[test]
    fn distinct_refusal_parts_are_reconciled_independently() {
        let item = json!({"id":"msg1","type":"message","content":[{"type":"refusal","refusal":"First"},{"type":"refusal","refusal":"Second"}]});
        let frames = project(vec![
            json!({"type":"response.refusal.delta","item_id":"msg1","output_index":1,"content_index":0,"delta":"First"}),
            json!({"type":"response.refusal.done","item_id":"msg1","output_index":1,"content_index":0,"refusal":"First"}),
            json!({"type":"response.refusal.delta","item_id":"msg1","output_index":1,"content_index":1,"delta":"Sec"}),
            json!({"type":"response.refusal.done","item_id":"msg1","output_index":1,"content_index":1,"refusal":"Second"}),
            json!({"type":"response.completed","response":{"output":[item]}}),
        ]);
        assert_eq!(delta_text(&frames, "refusal"), "FirstSecond");
    }

    #[test]
    fn top_level_error_message_is_preserved() {
        let error = ChatBridge::default()
            .convert(
                &json!({"type":"error","message":"quota exhausted","code":"quota"}),
                "r",
                "m",
                0,
            )
            .unwrap_err();
        assert_eq!(error, "quota exhausted");
    }

    #[test]
    fn content_part_done_is_reconciled_by_item_and_part() {
        let first = json!({"id":"msg1","type":"message","content":[{"type":"output_text","text":"A"},{"type":"output_text","text":"BC"}]});
        let second =
            json!({"id":"msg2","type":"message","content":[{"type":"output_text","text":"DE"}]});
        let frames = project(vec![
            json!({"type":"response.output_text.delta","item_id":"msg1","output_index":1,"content_index":1,"delta":"B"}),
            json!({"type":"response.content_part.done","item_id":"msg1","output_index":1,"content_index":1,"part":{"type":"output_text","text":"BC"}}),
            json!({"type":"response.output_item.done","output_index":1,"item":first}),
            json!({"type":"response.output_text.delta","item_id":"msg2","output_index":3,"content_index":0,"delta":"D"}),
            json!({"type":"response.completed","response":{"output":[first,second]}}),
        ]);
        // Events are projected in arrival order, with only the missing suffix
        // added when each part's full snapshot arrives.
        assert_eq!(delta_text(&frames, "content"), "BCADE");
    }

    #[test]
    fn contradictory_final_summary_is_an_error() {
        let mut bridge = ChatBridge::default();
        bridge.convert(&json!({"type":"response.reasoning_summary_text.delta","item_id":"rs1","delta":"Initial"}), "r", "m", 0).unwrap();
        assert!(bridge.convert(&json!({"type":"response.reasoning_summary_text.done","item_id":"rs1","text":"Changed"}), "r", "m", 0).is_err());
    }

    fn project(events: Vec<Value>) -> Vec<Value> {
        let mut bridge = ChatBridge::default();
        events
            .into_iter()
            .flat_map(|e| bridge.convert(&e, "r", "m", 0).unwrap())
            .collect()
    }
    #[test]
    fn done_only_arguments_are_complete_and_not_repeated() {
        let tool = json!({"type":"function_call","id":"fc1","call_id":"call1","name":"fetch","arguments":"{\"url\":\"https://example.com\"}"});
        let events = project(vec![
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc1","call_id":"call1","name":"fetch","arguments":""}}),
            json!({"type":"response.function_call_arguments.done","output_index":1,"arguments":tool["arguments"]}),
            json!({"type":"response.output_item.done","output_index":1,"item":tool}),
            json!({"type":"response.completed","response":{"status":"completed","output":[tool],"usage":{"input_tokens":3,"output_tokens":8,"output_tokens_details":{"reasoning_tokens":4}}}}),
        ]);
        let args = events
            .iter()
            .filter_map(|e| {
                e.pointer("/choices/0/delta/tool_calls/0/function/arguments")
                    .and_then(Value::as_str)
            })
            .collect::<String>();
        assert_eq!(
            serde_json::from_str::<Value>(&args).unwrap()["url"],
            "https://example.com"
        );
        assert_eq!(
            events
                .last()
                .unwrap()
                .pointer("/usage/completion_tokens_details/reasoning_tokens"),
            Some(&json!(4))
        );
    }
    #[test]
    fn partial_arguments_receive_only_missing_suffix() {
        let tool = json!({"type":"function_call","call_id":"c","name":"fetch","arguments":"{\"url\":\"u\"}"});
        let frames = project(vec![
            json!({"type":"response.output_item.added","item":{"type":"function_call","call_id":"c","name":"fetch","arguments":""}}),
            json!({"type":"response.function_call_arguments.delta","delta":"{\"url\":"}),
            json!({"type":"response.output_item.done","item":tool}),
            json!({"type":"response.completed","response":{"output":[tool]}}),
        ]);
        let args = frames
            .iter()
            .filter_map(|e| {
                e.pointer("/choices/0/delta/tool_calls/0/function/arguments")
                    .and_then(Value::as_str)
            })
            .collect::<String>();
        assert_eq!(args, "{\"url\":\"u\"}");
    }
    #[test]
    fn public_summary_deduplicates_and_opaque_reasoning_stays_private() {
        let item = json!({"type":"reasoning","id":"rs1","summary":[{"type":"summary_text","text":"Check docs"}],"encrypted_content":"opaque-secret"});
        let frames = project(vec![
            json!({"type":"response.reasoning_summary_text.delta","item_id":"rs1","delta":"Check "}),
            json!({"type":"response.reasoning_summary_text.done","item_id":"rs1","text":"Check docs"}),
            json!({"type":"response.output_item.done","item":item}),
            json!({"type":"response.completed","response":{"output":[item]}}),
        ]);
        let text = frames
            .iter()
            .filter_map(|e| {
                e.pointer("/choices/0/delta/reasoning_content")
                    .and_then(Value::as_str)
            })
            .collect::<String>();
        assert_eq!(text, "Check docs");
        assert!(!serde_json::to_string(&frames)
            .unwrap()
            .contains("opaque-secret"));
    }
    #[test]
    fn terminal_only_reply_and_calls_are_restored() {
        let frames = project(vec![
            json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"Answer"}]},{"type":"function_call","call_id":"c1","name":"fetch","arguments":"{}"},{"type":"function_call","call_id":"c2","name":"fetch","arguments":"{}"}]}}),
        ]);
        let ids = frames
            .iter()
            .filter_map(|frame| {
                frame
                    .pointer("/choices/0/delta/tool_calls/0/id")
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["c1", "c2"]);
        assert_eq!(
            frames.last().unwrap().pointer("/choices/0/finish_reason"),
            Some(&json!("tool_calls"))
        );
        assert!(frames
            .iter()
            .any(|e| e.pointer("/choices/0/delta/content") == Some(&json!("Answer"))));
    }
    #[test]
    fn failures_never_produce_a_success_terminal() {
        let mut bridge = ChatBridge::default();
        assert!(bridge
            .convert(&json!({"type":"response.incomplete"}), "r", "m", 0)
            .is_err());
        assert!(!bridge.completed);
        assert!(bridge
            .convert(
                &json!({"type":"response.incomplete","response":{"status":"failed"}}),
                "r",
                "m",
                0
            )
            .is_err());
        assert!(!bridge.completed);
    }
}
