use super::*;

#[test]
fn null_error_field_does_not_turn_a_valid_reply_into_failure() {
    let request = serde_json::json!({"model":"test"});
    for (protocol, body) in [
        (
            EngineProtocol::Anthropic,
            serde_json::json!({"error":null,"content":[{"type":"text","text":"Answer"}],"stop_reason":"end_turn"}),
        ),
        (
            EngineProtocol::Gemini,
            serde_json::json!({"error":null,"candidates":[{"content":{"parts":[{"text":"Answer"}]},"finishReason":"STOP"}]}),
        ),
    ] {
        assert_eq!(
            native_response_to_chat_completion(&body, &request, protocol).unwrap()["choices"][0]
                ["message"]["content"],
            "Answer"
        );
    }
}

#[test]
fn responses_usage_cannot_overflow_or_report_negative_counts() {
    let usage = responses_chat_usage(
        &serde_json::json!({"usage":{"input_tokens":i64::MAX,"output_tokens":i64::MAX}}),
    );
    assert_eq!(usage["total_tokens"], i64::MAX);
    let usage =
        responses_chat_usage(&serde_json::json!({"usage":{"input_tokens":-10,"output_tokens":12}}));
    assert_eq!(usage, serde_json::json!({"completion_tokens":12}));
}

#[test]
fn chat_request_converts_messages_and_tools() {
    let converted = chat_to_responses(&serde_json::json!({
            "model":"gpt-5.4",
            "messages":[
                {"role":"system","content":"Be concise"},
                {"role":"user","content":"hello"},
                {"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"call_1","content":"done"}
            ],
            "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}]
        })).unwrap();
    assert_eq!(converted["instructions"], "Be concise");
    assert_eq!(converted["input"].as_array().unwrap().len(), 3);
    assert_eq!(converted["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(converted["input"][1]["type"], "function_call");
    assert_eq!(converted["input"][2]["type"], "function_call_output");
    assert!(converted["input"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| { item.get("content") != Some(&serde_json::Value::Null) }));
    assert_eq!(converted["tools"][0]["name"], "lookup");
    assert_eq!(converted["stream"], true);
}

#[test]
fn chat_request_preserves_tool_policy_and_strict_schema() {
    let converted = chat_to_responses(&serde_json::json!({
        "model":"gpt-test",
        "messages":[{"role":"user","content":"lookup"}],
        "tools":[{"type":"function","function":{
            "name":"lookup","parameters":{"type":"object"},"strict":true
        }}],
        "tool_choice":{"type":"function","function":{"name":"lookup"}},
        "parallel_tool_calls":false
    }))
    .unwrap();
    assert_eq!(converted["tools"][0]["strict"], true);
    assert_eq!(
        converted["tool_choice"],
        serde_json::json!({"type":"function","name":"lookup"})
    );
    assert_eq!(converted["parallel_tool_calls"], false);
}

#[test]
fn chat_request_normalizes_pi_text_blocks_by_role() {
    let converted = chat_to_responses(&serde_json::json!({
        "model":"gpt-test",
        "messages":[
            {"role":"user","content":[{"type":"text","text":"question"}]},
            {"role":"assistant","content":[{"type":"text","text":"answer"}]}
        ]
    }))
    .unwrap();
    assert_eq!(converted["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(converted["input"][1]["content"][0]["type"], "output_text");
}

#[test]
fn chat_image_message_converts_nested_url_and_detail() {
    let converted = chat_to_responses(&serde_json::json!({
        "model":"test", "messages":[{"role":"user","content":[
            {"type":"text","text":"describe"},
            {"type":"image_url","image_url":{"url":"data:image/png;base64,YWJj","detail":"low"}}
        ]}]
    }))
    .unwrap();
    assert_eq!(
        converted["input"][0]["content"][1],
        serde_json::json!({
            "type":"input_image","image_url":"data:image/png;base64,YWJj","detail":"low"
        })
    );
}

#[test]
fn chat_reasoning_effort_and_public_summary_are_preserved() {
    let result=chat_to_responses(&serde_json::json!({"model":"configured","reasoning_effort":"medium","messages":[{"role":"user","content":"question"}]})).unwrap();
    assert_eq!(
        result["reasoning"],
        serde_json::json!({"effort":"medium","summary":"auto"})
    );
    let chat = response_to_chat_completion(
        &serde_json::json!({"output":[{"type":"reasoning","encrypted_content":"opaque-secret","summary":[{"type":"summary_text","text":"Public summary"}]},{"type":"message","content":[{"type":"output_text","text":"Answer"}]}]}),
        &serde_json::json!({"model":"configured"}),
    );
    assert_eq!(
        chat["choices"][0]["message"]["reasoning_content"],
        "Public summary"
    );
    assert!(!chat.to_string().contains("opaque-secret"));
}

#[test]
fn completed_response_converts_to_chat_completion() {
    let response = serde_json::json!({
        "id":"resp_1","created_at":123,"status":"completed",
        "output":[{"type":"message","content":[{"type":"output_text","text":"OK"}]}],
        "usage":{"input_tokens":2,"output_tokens":1}
    });
    let chat = response_to_chat_completion(&response, &serde_json::json!({"model":"gpt-5.4"}));
    assert_eq!(chat["choices"][0]["message"]["content"], "OK");
    assert_eq!(chat["choices"][0]["finish_reason"], "stop");
    assert_eq!(chat["usage"]["total_tokens"], 3);
}

#[test]
fn review_responses_unary_preserves_cache_and_reasoning_usage() {
    let response = serde_json::json!({"output":[],"usage":{
        "input_tokens":20,"output_tokens":12,
        "input_tokens_details":{"cached_tokens":10},
        "output_tokens_details":{"reasoning_tokens":8}
    }});
    let chat = response_to_chat_completion(&response, &serde_json::json!({"model":"test"}));
    assert_eq!(chat["usage"]["prompt_tokens_details"]["cached_tokens"], 10);
    assert_eq!(
        chat["usage"]["completion_tokens_details"]["reasoning_tokens"],
        8
    );
}

#[test]
fn review_native_tool_arguments_are_preserved_or_rejected_explicitly() {
    for arguments in [
        serde_json::json!({"city":"Tokyo"}),
        serde_json::json!("{\"city\":\"Tokyo\"}"),
    ] {
        let request = serde_json::json!({"model":"test","max_tokens":128,"messages":[
            {"role":"user","content":"lookup"},
            {"role":"assistant","content":null,"tool_calls":[{"id":"a","type":"function","function":{"name":"lookup","arguments":arguments}}]}
        ]});
        let anthropic = chat_to_anthropic(&request).unwrap();
        let (gemini, _) = chat_to_gemini(&request).unwrap();
        assert_eq!(
            anthropic["messages"][1]["content"][0]["input"]["city"],
            "Tokyo"
        );
        assert_eq!(
            gemini["contents"][1]["parts"][0]["functionCall"]["args"]["city"],
            "Tokyo"
        );
    }
    for arguments in [
        serde_json::json!("{broken"),
        serde_json::json!("[]"),
        serde_json::json!(null),
    ] {
        let request = serde_json::json!({"model":"test","max_tokens":128,"messages":[
            {"role":"assistant","content":null,"tool_calls":[{"id":"a","type":"function","function":{"name":"lookup","arguments":arguments}}]}
        ]});
        assert!(matches!(
            chat_to_anthropic(&request),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            chat_to_gemini(&request),
            Err(AppError::BadRequest(_))
        ));
    }
}

#[test]
fn chat_request_converts_to_anthropic_messages() {
    let converted = chat_to_anthropic(&serde_json::json!({
            "model":"claude-sonnet-4-5",
            "messages":[
                {"role":"system","content":"Be concise"},
                {"role":"user","content":"hi"},
                {"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"q\":1}"}}]},
                {"role":"tool","tool_call_id":"call_1","content":"done"},
                {"role":"user","content":"bye"}
            ],
            "stream":true,
            "max_tokens":128
        }))
        .unwrap();
    assert_eq!(converted["model"], "claude-sonnet-4-5");
    assert_eq!(converted["system"], "Be concise");
    assert_eq!(converted["max_tokens"], 128);
    assert_eq!(converted["stream"], true);
    let messages = converted["messages"].as_array().unwrap();
    // user(hi) + assistant(text-less tool_use) + user(tool_result + bye merged)
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"][0]["type"], "text");
    assert_eq!(messages[1]["content"][0]["type"], "tool_use");
    assert_eq!(messages[1]["content"][0]["input"]["q"], 1);
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    assert_eq!(messages[2]["content"][1]["type"], "text");
    assert!(anthropic_messages_url("https://api.anthropic.com")
        .unwrap()
        .ends_with("/v1/messages"));
    assert!(anthropic_messages_url("https://proxy.example.com/v1/")
        .unwrap()
        .ends_with("/v1/messages"));
}

#[test]
fn anthropic_response_converts_to_chat_completion() {
    let chat = anthropic_to_chat_completion(
        &serde_json::json!({
            "id":"msg_1",
            "content":[
                {"type":"text","text":"Hello"},
                {"type":"tool_use","id":"toolu_1","name":"lookup","input":{"q":2}}
            ],
            "stop_reason":"tool_use",
            "usage":{"input_tokens":4,"output_tokens":2}
        }),
        &serde_json::json!({"model":"claude-sonnet-4-5"}),
    );
    assert_eq!(chat["id"], "msg_1");
    assert_eq!(chat["choices"][0]["message"]["content"], "Hello");
    assert_eq!(chat["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(
        chat["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        "{\"q\":2}"
    );
    assert_eq!(chat["usage"]["total_tokens"], 6);
}

#[test]
fn chat_request_converts_to_gemini_contents() {
    let (body, stream) = chat_to_gemini(&serde_json::json!({
            "model":"gemini-2.5-pro",
            "messages":[
                {"role":"system","content":"Be nice"},
                {"role":"user","content":"hi"},
                {"role":"assistant","tool_calls":[{"id":"c1","type":"function","function":{"name":"lookup","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"c1","content":"ok"}
            ],
            "stream":true
        }))
        .unwrap();
    assert!(stream);
    assert_eq!(body["systemInstruction"]["parts"][0]["text"], "Be nice");
    let contents = body["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 3);
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(contents[1]["parts"][0]["functionCall"]["name"], "lookup");
    assert_eq!(contents[2]["role"], "user");
    assert_eq!(
        contents[2]["parts"][0]["functionResponse"]["name"],
        "lookup"
    );
    assert_eq!(contents[2]["parts"][0]["functionResponse"]["id"], "c1");
    let url = gemini_generate_url(
        "https://generativelanguage.googleapis.com",
        "gemini-2.5-pro",
        true,
    )
    .unwrap();
    assert_eq!(
        url,
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
    );
    assert!(gemini_generate_url("https://example.com", "bad model", false).is_err());
}

#[test]
fn gemini_response_converts_to_chat_completion() {
    let chat = gemini_to_chat_completion(
        &serde_json::json!({
            "candidates":[{
                "content":{"parts":[{"text":"Hey"}]},
                "finishReason":"STOP"
            }],
            "usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}
        }),
        &serde_json::json!({"model":"gemini-2.5-pro"}),
    );
    assert_eq!(chat["choices"][0]["message"]["content"], "Hey");
    assert_eq!(chat["choices"][0]["finish_reason"], "stop");
    assert_eq!(chat["usage"]["total_tokens"], 5);
}

#[test]
fn review_gemini_unary_summary_usage_and_tool_result_round_trip() {
    let chat = gemini_to_chat_completion(
        &serde_json::json!({
            "candidates":[{"content":{"parts":[{"thought":true,"text":"Summary"},{"text":"Answer"},{"functionCall":{"id":"a","name":"lookup","args":{}},"thoughtSignature":"opaque"}]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"thoughtsTokenCount":7,"totalTokenCount":22}
        }),
        &serde_json::json!({"model":"test"}),
    );
    assert_eq!(chat["choices"][0]["message"]["content"], "Answer");
    assert_eq!(
        chat["choices"][0]["message"]["reasoning_content"],
        "Summary"
    );
    assert_eq!(chat["usage"]["completion_tokens"], 12);
    let (request, _) = chat_to_gemini(&serde_json::json!({"model":"test","messages":[
        {"role":"user","content":"lookup"},chat["choices"][0]["message"],
        {"role":"tool","tool_call_id":"a","content":"result"}
    ]}))
    .unwrap();
    assert_eq!(
        request["contents"][1]["parts"][0],
        serde_json::json!({"thought":true,"text":"Summary"})
    );
    assert_eq!(
        request["contents"][1]["parts"][2]["thoughtSignature"],
        "opaque"
    );
    assert_eq!(
        request["contents"][2]["parts"][0]["functionResponse"]["name"],
        "lookup"
    );
    assert_eq!(
        request["contents"][2]["parts"][0]["functionResponse"]["id"],
        "a"
    );
}

#[test]
fn public_reasoning_summary_is_preserved_only_by_gemini() {
    let payload = serde_json::json!({
        "model":"test",
        "max_tokens":16,
        "messages":[
            {"role":"user","content":"question"},
            {"role":"assistant","content":"answer","reasoning_content":"public summary"}
        ]
    });
    assert!(chat_to_responses(&payload).is_err());
    assert!(chat_to_anthropic(&payload).is_err());
    let (gemini, _) = chat_to_gemini(&payload).unwrap();
    assert_eq!(
        gemini["contents"][1]["parts"],
        serde_json::json!([
            {"thought":true,"text":"public summary"},
            {"text":"answer"}
        ])
    );
}

#[test]
fn upstream_protocol_errors_map_to_openai_error_shape() {
    let anthropic = protocol_error_to_openai(serde_json::json!({
        "type":"error","error":{"type":"overloaded_error","message":"Overloaded"}
    }));
    assert_eq!(anthropic["error"]["message"], "Overloaded");
    assert_eq!(anthropic["error"]["type"], "overloaded_error");
    let gemini = protocol_error_to_openai(serde_json::json!({
        "error":{"code":429,"message":"quota","status":"RESOURCE_EXHAUSTED"}
    }));
    assert_eq!(gemini["error"]["message"], "quota");
    assert_eq!(gemini["error"]["type"], "RESOURCE_EXHAUSTED");
}

#[test]
fn review_native_unary_errors_and_blocked_prompts_are_distinguished() {
    for protocol in [EngineProtocol::Anthropic, EngineProtocol::Gemini] {
        for body in [
            serde_json::json!({"error":{"message":"upstream error"}}),
            serde_json::json!({}),
        ] {
            assert!(matches!(
                native_response_to_chat_completion(
                    &body,
                    &serde_json::json!({"model":"test"}),
                    protocol
                ),
                Err(AppError::BadGateway(_))
            ));
        }
    }
    let chat = native_response_to_chat_completion(
        &serde_json::json!({"promptFeedback":{"blockReason":"SAFETY"}}),
        &serde_json::json!({"model":"test"}),
        EngineProtocol::Gemini,
    )
    .unwrap();
    assert_eq!(chat["choices"][0]["finish_reason"], "content_filter");
}

#[test]
fn anthropic_unary_usage_includes_cache_and_preserves_details() {
    let result = anthropic_to_chat_completion(
        &serde_json::json!({
            "content":[{"type":"text","text":"hello"}],
            "usage":{"input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":20,"output_tokens":5}
        }),
        &serde_json::json!({"model":"test"}),
    );
    assert_eq!(
        result["usage"],
        serde_json::json!({
            "prompt_tokens":130,"completion_tokens":5,"total_tokens":135,
            "prompt_tokens_details":{"cached_tokens":100,"cache_write_tokens":20}
        })
    );
    assert_eq!(
        anthropic_chat_usage(
            &serde_json::json!({"input_tokens":i64::MAX,"cache_read_input_tokens":3,"output_tokens":-2})
        )["prompt_tokens"],
        i64::MAX
    );
}

#[test]
fn missing_and_partial_usage_is_never_fabricated_as_zero() {
    for source in [
        serde_json::json!({}),
        Value::Null,
        serde_json::json!({"input_tokens":"unknown"}),
    ] {
        assert!(responses_chat_usage(&serde_json::json!({"usage":source})).is_null());
        assert!(anthropic_chat_usage(&source).is_null());
        assert!(gemini_chat_usage(&source).is_null());
    }
    assert_eq!(
        responses_chat_usage(&serde_json::json!({"usage":{"input_tokens":7}})),
        serde_json::json!({"prompt_tokens":7})
    );
    assert_eq!(
        anthropic_chat_usage(&serde_json::json!({"output_tokens":3})),
        serde_json::json!({"completion_tokens":3})
    );
    assert_eq!(
        gemini_chat_usage(&serde_json::json!({"promptTokenCount":9})),
        serde_json::json!({"prompt_tokens":9})
    );
    assert_eq!(
        gemini_chat_usage(&serde_json::json!({"promptTokenCount":9,"totalTokenCount":12})),
        serde_json::json!({"prompt_tokens":9,"completion_tokens":3,"total_tokens":12})
    );
}
