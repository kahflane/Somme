//! Any OpenAI-compatible chat server, through the `async-openai` library:
//! OpenAI, Gemini, Anthropic's compatible endpoint, DeepSeek, Qwen, Kimi,
//! GLM and many others, or Ollama and LM Studio on this computer. Requests
//! are built with the library's types. Answers are read loosely, since
//! servers differ in the fields they send back.

use std::collections::BTreeMap;
use std::time::Duration;

use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::error::OpenAIError;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls, ChatCompletionRequestAssistantMessage,
    ChatCompletionRequestAssistantMessageContent, ChatCompletionRequestMessage, ChatCompletionRequestMessageContentPartImage,
    ChatCompletionRequestMessageContentPartText, ChatCompletionRequestSystemMessage, ChatCompletionRequestSystemMessageContent,
    ChatCompletionRequestToolMessage, ChatCompletionRequestToolMessageContent, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, ChatCompletionRequestUserMessageContentPart, ChatCompletionTool, ChatCompletionTools,
    CreateChatCompletionRequest, FunctionCall, FunctionObject, ImageUrl, ResponseFormat,
};
use base64::Engine as _;
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AiError, excerpt};

const SERVICE: &str = "The LLM";

/// Where a provider's servers are, for grouping the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Global,
    China,
    /// On this computer.
    Local,
}

/// A provider people often use, to fill in the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub name: &'static str,
    pub base_url: &'static str,
    /// Local servers need no key.
    pub needs_key: bool,
    pub region: Region,
}

const fn preset(name: &'static str, base_url: &'static str, region: Region) -> Preset {
    Preset { name, base_url, needs_key: !matches!(region, Region::Local), region }
}

pub const PRESETS: &[Preset] = &[
    preset("OpenAI", "https://api.openai.com/v1", Region::Global),
    preset("Anthropic", "https://api.anthropic.com/v1", Region::Global),
    preset("Google Gemini", "https://generativelanguage.googleapis.com/v1beta/openai", Region::Global),
    preset("OpenRouter", "https://openrouter.ai/api/v1", Region::Global),
    preset("Groq", "https://api.groq.com/openai/v1", Region::Global),
    preset("DeepSeek", "https://api.deepseek.com/v1", Region::China),
    preset("Qwen (Alibaba Cloud)", "https://dashscope-intl.aliyuncs.com/compatible-mode/v1", Region::China),
    preset("Qwen (Alibaba Cloud China)", "https://dashscope.aliyuncs.com/compatible-mode/v1", Region::China),
    preset("Kimi (Moonshot)", "https://api.moonshot.ai/v1", Region::China),
    preset("Kimi (Moonshot China)", "https://api.moonshot.cn/v1", Region::China),
    preset("GLM (Z.ai)", "https://api.z.ai/api/paas/v4", Region::China),
    preset("GLM (Zhipu China)", "https://open.bigmodel.cn/api/paas/v4", Region::China),
    preset("MiniMax", "https://api.minimax.io/v1", Region::China),
    preset("MiniMax (China)", "https://api.minimaxi.com/v1", Region::China),
    preset("Doubao (Volcengine Ark)", "https://ark.cn-beijing.volces.com/api/v3", Region::China),
    preset("Hunyuan (Tencent)", "https://api.hunyuan.cloud.tencent.com/v1", Region::China),
    preset("ERNIE (Baidu Qianfan)", "https://qianfan.baidubce.com/v2", Region::China),
    preset("SiliconFlow", "https://api.siliconflow.cn/v1", Region::China),
    preset("StepFun", "https://api.stepfun.com/v1", Region::China),
    preset("Yi (01.AI)", "https://api.lingyiwanwu.com/v1", Region::China),
    preset("Baichuan", "https://api.baichuan-ai.com/v1", Region::China),
    preset("Ollama", "http://localhost:11434/v1", Region::Local),
    preset("LM Studio", "http://localhost:1234/v1", Region::Local),
];

/// The preset with this address, if any.
pub fn preset_for(base_url: &str) -> Option<&'static Preset> {
    let base = base_url.trim().trim_end_matches('/');
    PRESETS.iter().find(|preset| preset.base_url.eq_ignore_ascii_case(base))
}

/// Whether `base_url` points at this computer, so nothing leaves it.
pub fn is_local(base_url: &str) -> bool {
    let rest = base_url.split_once("://").map_or(base_url, |(_, rest)| rest);
    let host = rest.split(['/', '?']).next().unwrap_or_default();
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => host.rsplit_once(':').map_or(host, |(name, _)| name),
    };
    let host = host.to_ascii_lowercase();
    host == "localhost" || host == "::1" || host.starts_with("127.") || host.ends_with(".localhost")
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Up to the version, like `https://api.openai.com/v1`.
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
}

impl LlmConfig {
    pub fn is_set_up(&self) -> bool {
        !self.base_url.trim().is_empty() && !self.model.trim().is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Text(String),
    /// Sent inline, like a screenshot.
    Image { media_type: String, data: Vec<u8> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
    /// Tools the assistant asked for.
    pub tool_calls: Vec<ToolCall>,
    /// For a tool's result: the call it answers.
    pub tool_call_id: Option<String>,
}

impl Message {
    fn text(role: Role, text: impl Into<String>) -> Self {
        Self { role, parts: vec![Part::Text(text.into())], tool_calls: Vec::new(), tool_call_id: None }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::text(Role::System, text)
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::text(Role::User, text)
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::text(Role::Assistant, text)
    }

    /// The assistant's turn as it came back, tool calls and all.
    pub fn from_reply(reply: &ChatReply) -> Self {
        let parts = if reply.text.is_empty() { Vec::new() } else { vec![Part::Text(reply.text.clone())] };
        Self { role: Role::Assistant, parts, tool_calls: reply.tool_calls.clone(), tool_call_id: None }
    }

    pub fn tool_result(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self { tool_call_id: Some(call_id.into()), ..Self::text(Role::Tool, text) }
    }

    /// A user turn with a picture, as a PNG.
    pub fn user_with_png(text: impl Into<String>, png: Vec<u8>) -> Self {
        Self {
            role: Role::User,
            parts: vec![Part::Text(text.into()), Part::Image { media_type: "image/png".into(), data: png }],
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// The text parts, joined.
    pub fn plain_text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|part| match part {
                Part::Text(text) => Some(text.as_str()),
                Part::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn to_openai(&self) -> ChatCompletionRequestMessage {
        let text = self.plain_text();
        match self.role {
            Role::System => ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
                content: ChatCompletionRequestSystemMessageContent::Text(text),
                name: None,
            }),
            Role::User if self.parts.iter().any(|part| matches!(part, Part::Image { .. })) => {
                let parts = self.parts.iter().map(|part| match part {
                    Part::Text(text) => ChatCompletionRequestUserMessageContentPart::Text(ChatCompletionRequestMessageContentPartText {
                        text: text.clone(),
                        ..Default::default()
                    }),
                    Part::Image { media_type, data } => {
                        let encoded = base64::engine::general_purpose::STANDARD.encode(data);
                        ChatCompletionRequestUserMessageContentPart::ImageUrl(ChatCompletionRequestMessageContentPartImage {
                            image_url: ImageUrl { url: format!("data:{media_type};base64,{encoded}"), detail: None },
                            ..Default::default()
                        })
                    }
                });
                ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                    content: ChatCompletionRequestUserMessageContent::Array(parts.collect()),
                    name: None,
                })
            }
            Role::User => ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                content: ChatCompletionRequestUserMessageContent::Text(text),
                name: None,
            }),
            Role::Assistant => {
                let calls = self.tool_calls.iter().map(|call| {
                    ChatCompletionMessageToolCalls::Function(ChatCompletionMessageToolCall {
                        id: call.id.clone(),
                        function: FunctionCall { name: call.name.clone(), arguments: call.arguments.clone() },
                    })
                });
                ChatCompletionRequestMessage::Assistant(ChatCompletionRequestAssistantMessage {
                    // Only tool calls: no content at all.
                    content: (!self.parts.is_empty() || self.tool_calls.is_empty())
                        .then_some(ChatCompletionRequestAssistantMessageContent::Text(text)),
                    tool_calls: (!self.tool_calls.is_empty()).then(|| calls.collect()),
                    ..Default::default()
                })
            }
            Role::Tool => ChatCompletionRequestMessage::Tool(ChatCompletionRequestToolMessage {
                content: ChatCompletionRequestToolMessageContent::Text(text),
                tool_call_id: self.tool_call_id.clone().unwrap_or_default(),
            }),
        }
    }
}

/// A tool the model may call. `parameters` is a JSON schema.
#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON text, as the model wrote it.
    pub arguments: String,
}

impl ToolCall {
    pub fn args(&self) -> Value {
        serde_json::from_str(&self.arguments).unwrap_or(Value::Null)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChatRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<Tool>,
    /// Asks for a JSON object back, where the server supports it.
    pub json: bool,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatReply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
}

pub struct Llm {
    client: Client<OpenAIConfig>,
    model: String,
}

impl Llm {
    pub fn new(config: &LlmConfig) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(180))
            .build()
            .unwrap_or_default();
        let base = config.base_url.trim().trim_end_matches('/');
        let key = config.api_key.as_deref().map(str::trim).filter(|key| !key.is_empty()).unwrap_or_default();
        let mut settings = OpenAIConfig::new().with_api_base(base).with_api_key(key);
        // Anthropic's own endpoints, like the model list, want the key here.
        if base.to_ascii_lowercase().contains("://api.anthropic.com") && !key.is_empty() {
            let headers = settings.clone().with_header("x-api-key", key).and_then(|with| with.with_header("anthropic-version", "2023-06-01"));
            if let Ok(with) = headers {
                settings = with;
            }
        }
        Self { client: Client::build(http, settings), model: config.model.trim().to_owned() }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    #[allow(deprecated)] // max_tokens, which more servers know than its newer name.
    fn body(&self, request: &ChatRequest, stream: bool, json_mode: bool) -> CreateChatCompletionRequest {
        let tools = request.tools.iter().map(|tool| {
            ChatCompletionTools::Function(ChatCompletionTool {
                function: FunctionObject {
                    name: tool.name.clone(),
                    description: Some(tool.description.clone()),
                    parameters: Some(tool.parameters.clone()),
                    strict: None,
                },
            })
        });
        CreateChatCompletionRequest {
            model: self.model.clone(),
            messages: request.messages.iter().map(Message::to_openai).collect(),
            tools: (!request.tools.is_empty()).then(|| tools.collect()),
            response_format: json_mode.then_some(ResponseFormat::JsonObject),
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            stream: stream.then_some(true),
            ..Default::default()
        }
    }

    /// Asks for JSON, and asks again without when the server refuses it.
    /// Some refuse with an error the library can't read, so any refusal counts.
    fn json_refused(request: &ChatRequest, err: &AiError) -> bool {
        request.json && matches!(err, AiError::Status { status: 400 | 422, .. } | AiError::Answer { .. })
    }

    pub async fn chat(&self, request: &ChatRequest) -> Result<ChatReply, AiError> {
        let send = |json_mode| async move {
            let answer: Value = self.client.chat().create_byot(self.body(request, false, json_mode)).await.map_err(ai_error)?;
            reply_from(&answer)
        };
        match send(request.json).await {
            Err(err) if Self::json_refused(request, &err) => send(false).await,
            other => other,
        }
    }

    /// Like `chat`, giving the text to `on_text` as it arrives.
    pub async fn chat_streaming(&self, request: &ChatRequest, mut on_text: impl FnMut(&str) + Send) -> Result<ChatReply, AiError> {
        let open = |json_mode| async move { self.client.chat().create_stream_byot::<_, Value>(self.body(request, true, json_mode)).await };
        let mut events = match open(request.json).await.map_err(ai_error) {
            Err(err) if Self::json_refused(request, &err) => open(false).await.map_err(ai_error)?,
            other => other?,
        };
        let mut reply = ChatReply::default();
        let mut calls: BTreeMap<u64, ToolCall> = BTreeMap::new();
        while let Some(event) = events.next().await {
            let event = event.map_err(ai_error)?;
            if let Some(error) = event.get("error") {
                return Err(AiError::answer(SERVICE, excerpt(&error.to_string())));
            }
            read_usage(&event, &mut reply.usage);
            let delta = &event["choices"][0]["delta"];
            if let Some(text) = delta["content"].as_str().filter(|text| !text.is_empty()) {
                reply.text.push_str(text);
                on_text(text);
            }
            for part in delta["tool_calls"].as_array().into_iter().flatten() {
                let index = part["index"].as_u64().unwrap_or(calls.len() as u64);
                let call = calls.entry(index).or_insert_with(|| ToolCall {
                    id: String::new(),
                    name: String::new(),
                    arguments: String::new(),
                });
                if let Some(id) = part["id"].as_str() {
                    call.id = id.to_owned();
                }
                if let Some(name) = part["function"]["name"].as_str() {
                    call.name.push_str(name);
                }
                if let Some(arguments) = part["function"]["arguments"].as_str() {
                    call.arguments.push_str(arguments);
                }
            }
        }
        reply.tool_calls = calls
            .into_values()
            .enumerate()
            .map(|(ix, mut call)| {
                if call.id.is_empty() {
                    call.id = format!("call_{ix}");
                }
                call
            })
            .collect();
        Ok(reply)
    }

    /// The models the server offers, for the settings.
    pub async fn models(&self) -> Result<Vec<String>, AiError> {
        let answer: Value = self.client.models().list_byot().await.map_err(ai_error)?;
        let mut names: Vec<String> = answer["data"]
            .as_array()
            .or_else(|| answer["models"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|model| model["id"].as_str().or_else(|| model["name"].as_str()))
            // Gemini names them `models/gemini-…`; chat wants the name alone.
            .map(|name| name.strip_prefix("models/").unwrap_or(name).to_owned())
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Checks the address, key and model with a tiny question.
    pub async fn test(&self) -> Result<String, AiError> {
        let request = ChatRequest {
            messages: vec![Message::user("Reply with the single word OK.")],
            max_tokens: Some(20),
            ..ChatRequest::default()
        };
        let reply = self.chat(&request).await?;
        Ok(reply.text.trim().to_owned())
    }
}

/// The library's error, in words the user can act on.
fn ai_error(err: OpenAIError) -> AiError {
    match err {
        OpenAIError::Reqwest(err) => AiError::network(SERVICE, err),
        OpenAIError::ApiError(err) => AiError::Status {
            service: SERVICE,
            status: err.status_code.as_u16(),
            body: excerpt(&err.api_error.message),
        },
        OpenAIError::JSONDeserialize(_, content) => AiError::answer(SERVICE, excerpt(&content)),
        other => AiError::answer(SERVICE, excerpt(&other.to_string())),
    }
}

fn read_usage(value: &Value, usage: &mut Usage) {
    let found = &value["usage"];
    if let Some(input) = found["prompt_tokens"].as_u64() {
        usage.input_tokens = input;
    }
    if let Some(output) = found["completion_tokens"].as_u64() {
        usage.output_tokens = output;
    }
}

fn reply_from(value: &Value) -> Result<ChatReply, AiError> {
    if let Some(error) = value.get("error") {
        return Err(AiError::answer(SERVICE, excerpt(&error.to_string())));
    }
    let message = &value["choices"][0]["message"];
    if message.is_null() {
        return Err(AiError::answer(SERVICE, "no choices in the answer"));
    }
    let mut reply = ChatReply { text: message["content"].as_str().unwrap_or_default().to_owned(), ..ChatReply::default() };
    read_usage(value, &mut reply.usage);
    reply.tool_calls = message["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(ix, call)| ToolCall {
            id: call["id"].as_str().map_or_else(|| format!("call_{ix}"), str::to_owned),
            name: call["function"]["name"].as_str().unwrap_or_default().to_owned(),
            arguments: match &call["function"]["arguments"] {
                Value::String(text) => text.clone(),
                Value::Null => "{}".to_owned(),
                other => other.to_string(),
            },
        })
        .collect();
    Ok(reply)
}

/// The JSON in a model's answer: the whole text, a fenced block, or the
/// outermost braces.
pub fn extract_json(text: &str) -> Option<Value> {
    let text = text.trim();
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    if let Some(start) = text.find("```") {
        let rest = &text[start + 3..];
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        if let Some(end) = rest.find("```")
            && let Ok(value) = serde_json::from_str(rest[..end].trim())
        {
            return Some(value);
        }
    }
    for (open, close) in [('{', '}'), ('[', ']')] {
        if let (Some(start), Some(end)) = (text.find(open), text.rfind(close))
            && start < end
            && let Ok(value) = serde_json::from_str(&text[start..=end])
        {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::test_server::serve;

    fn config(base: &str) -> LlmConfig {
        LlmConfig { base_url: format!("{base}/v1/"), api_key: Some("sk-test".into()), model: "test-model".into() }
    }

    #[test]
    fn local_servers_are_known() {
        assert!(is_local("http://localhost:11434/v1"));
        assert!(is_local("http://127.0.0.1:1234/v1"));
        assert!(is_local("http://[::1]:8080/v1"));
        assert!(!is_local("https://api.openai.com/v1"));
        assert!(!is_local("https://localhost.example.test/v1"));
    }

    fn to_json(message: &Message) -> Value {
        serde_json::to_value(message.to_openai()).unwrap()
    }

    #[test]
    fn presets_are_found_by_address() {
        assert_eq!(preset_for("https://api.deepseek.com/v1/").map(|preset| preset.name), Some("DeepSeek"));
        assert!(PRESETS.iter().any(|preset| preset.name == "Google Gemini"));
        assert!(!preset_for("http://localhost:11434/v1").unwrap().needs_key);
        assert_eq!(preset_for("https://example.test/v1"), None);
    }

    #[test]
    fn messages_are_written_as_openai_wants() {
        let screenshot = Message::user_with_png("What is on screen?", vec![1, 2, 3]);
        let json = to_json(&screenshot);
        assert_eq!(json["content"][1]["image_url"]["url"], "data:image/png;base64,AQID");
        let call = ChatReply {
            text: String::new(),
            tool_calls: vec![ToolCall { id: "c1".into(), name: "run".into(), arguments: "{}".into() }],
            usage: Usage::default(),
        };
        let json = to_json(&Message::from_reply(&call));
        assert_eq!(json["content"], Value::Null);
        assert_eq!(json["tool_calls"][0]["function"]["name"], "run");
        assert_eq!(to_json(&Message::tool_result("c1", "done"))["tool_call_id"], "c1");
    }

    #[test]
    fn json_is_found_in_chatty_answers() {
        assert_eq!(extract_json(r#"{"a":1}"#), Some(json!({"a":1})));
        assert_eq!(extract_json("Sure!\n```json\n{\"a\": 2}\n```"), Some(json!({"a":2})));
        assert_eq!(extract_json("Here: {\"a\": 3} hope it helps"), Some(json!({"a":3})));
        assert_eq!(extract_json("no json"), None);
    }

    #[tokio::test]
    async fn chat_reads_text_and_tool_calls() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"Hi","tool_calls":[
            {"id":"call_1","type":"function","function":{"name":"run_command","arguments":"{\"command\":\"ls\"}"}}]}}],
            "usage":{"prompt_tokens":12,"completion_tokens":5}}"#;
        let (address, server) = serve(vec![(200, body.into())]).await;
        let llm = Llm::new(&config(&address));
        let request = ChatRequest {
            messages: vec![Message::user("hello")],
            tools: vec![Tool { name: "run_command".into(), description: "Runs".into(), parameters: json!({"type":"object"}) }],
            ..ChatRequest::default()
        };
        let reply = llm.chat(&request).await.unwrap();
        assert_eq!(reply.text, "Hi");
        assert_eq!(reply.tool_calls[0].args()["command"], "ls");
        assert_eq!(reply.usage, Usage { input_tokens: 12, output_tokens: 5 });
        let seen = server.await.unwrap();
        assert!(seen[0].starts_with("POST /v1/chat/completions"), "{}", seen[0]);
        assert!(seen[0].contains(r#""model":"test-model""#));
    }

    #[tokio::test]
    async fn models_are_listed_by_name() {
        let body = r#"{"object":"list","data":[{"id":"models/gemini-2.5-flash"},{"id":"gemini-2.5-pro","object":"model"},{"id":"gemini-2.5-pro"}]}"#;
        let (address, server) = serve(vec![(200, body.into())]).await;
        let models = Llm::new(&config(&address)).models().await.unwrap();
        assert_eq!(models, ["gemini-2.5-flash", "gemini-2.5-pro"]);
        let seen = server.await.unwrap();
        assert!(seen[0].starts_with("GET /v1/models"), "{}", seen[0]);
        assert!(seen[0].to_ascii_lowercase().contains("authorization: bearer sk-test"));
    }

    #[tokio::test]
    async fn json_mode_is_dropped_when_refused() {
        let ok = r#"{"choices":[{"message":{"content":"{\"x\":1}"}}]}"#;
        let (address, server) = serve(vec![(400, r#"{"error":"response_format not supported"}"#.into()), (200, ok.into())]).await;
        let llm = Llm::new(&config(&address));
        let request = ChatRequest { messages: vec![Message::user("json please")], json: true, ..ChatRequest::default() };
        let reply = llm.chat(&request).await.unwrap();
        assert_eq!(extract_json(&reply.text), Some(json!({"x":1})));
        let seen = server.await.unwrap();
        assert!(seen[0].contains("response_format"));
        assert!(!seen[1].contains("response_format"));
    }

    #[tokio::test]
    async fn streaming_joins_text_and_tool_call_pieces() {
        let events = [
            r#"{"choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"{"choices":[{"delta":{"content":"lo"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c9","function":{"name":"click","arguments":"{\"x\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"5}"}}]}}]}"#,
        ];
        let mut body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
        body.push_str("data: [DONE]\n\n");
        let (address, _server) = serve(vec![(200, body)]).await;
        let llm = Llm::new(&config(&address));
        let mut seen = String::new();
        let request = ChatRequest { messages: vec![Message::user("hi")], ..ChatRequest::default() };
        let reply = llm.chat_streaming(&request, |text| seen.push_str(text)).await.unwrap();
        assert_eq!(seen, "Hello");
        assert_eq!(reply.text, "Hello");
        assert_eq!(reply.tool_calls, vec![ToolCall { id: "c9".into(), name: "click".into(), arguments: "{\"x\":5}".into() }]);
    }
}
