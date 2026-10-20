//! Chat with an LLM through the `genai` library. It speaks each provider's
//! own API: OpenAI, Gemini, Anthropic, DeepSeek, Qwen, Kimi, GLM and many
//! more, and keeps what each needs between turns, like Gemini's thought
//! signatures and Anthropic's thinking. A server it has no adapter for is
//! reached the OpenAI way, as most are, like Ollama and LM Studio on this
//! computer.

use std::time::Duration;

use base64::Engine as _;
use futures::StreamExt as _;
use genai::adapter::AdapterKind;
use genai::chat::{
    self as genai_chat, Binary, CacheControl, ChatMessage, ChatOptions, ChatResponseFormat, ChatStreamEvent, ContentPart, MessageContent, ReasoningEffort,
    ToolResponse,
};
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ModelIden, ServiceTarget};
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
    /// How genai talks to it.
    adapter: AdapterKind,
}

const fn preset(name: &'static str, base_url: &'static str, region: Region, adapter: AdapterKind) -> Preset {
    Preset { name, base_url, needs_key: !matches!(region, Region::Local), region, adapter }
}

pub const PRESETS: &[Preset] = &[
    preset("OpenAI", "https://api.openai.com/v1", Region::Global, AdapterKind::OpenAI),
    preset("Anthropic", "https://api.anthropic.com/v1", Region::Global, AdapterKind::Anthropic),
    preset("Google Gemini", "https://generativelanguage.googleapis.com/v1beta", Region::Global, AdapterKind::Gemini),
    preset("xAI", "https://api.x.ai/v1", Region::Global, AdapterKind::Xai),
    preset("Mistral", "https://api.mistral.ai/v1", Region::Global, AdapterKind::Mistral),
    preset("OpenRouter", "https://openrouter.ai/api/v1", Region::Global, AdapterKind::OpenRouter),
    preset("Groq", "https://api.groq.com/openai/v1", Region::Global, AdapterKind::Groq),
    preset("DeepSeek", "https://api.deepseek.com/v1", Region::China, AdapterKind::DeepSeek),
    preset("Qwen (Alibaba Cloud)", "https://dashscope-intl.aliyuncs.com/compatible-mode/v1", Region::China, AdapterKind::QwenCloud),
    preset("Qwen (Alibaba Cloud China)", "https://dashscope.aliyuncs.com/compatible-mode/v1", Region::China, AdapterKind::Aliyun),
    preset("Kimi (Moonshot)", "https://api.moonshot.ai/v1", Region::China, AdapterKind::Kimi),
    preset("Kimi (Moonshot China)", "https://api.moonshot.cn/v1", Region::China, AdapterKind::Moonshot),
    preset("GLM (Z.ai)", "https://api.z.ai/api/paas/v4", Region::China, AdapterKind::Zai),
    preset("GLM (Zhipu China)", "https://open.bigmodel.cn/api/paas/v4", Region::China, AdapterKind::BigModel),
    preset("MiniMax", "https://api.minimax.io/anthropic/v1", Region::China, AdapterKind::MiniMax),
    preset("MiniMax (China)", "https://api.minimaxi.com/anthropic/v1", Region::China, AdapterKind::MiniMax),
    preset("Doubao (Volcengine Ark)", "https://ark.cn-beijing.volces.com/api/v3", Region::China, AdapterKind::OpenAI),
    preset("Hunyuan (Tencent)", "https://api.hunyuan.cloud.tencent.com/v1", Region::China, AdapterKind::OpenAI),
    preset("ERNIE (Baidu Qianfan)", "https://qianfan.baidubce.com/v2", Region::China, AdapterKind::Baidu),
    preset("SiliconFlow", "https://api.siliconflow.cn/v1", Region::China, AdapterKind::OpenAI),
    preset("StepFun", "https://api.stepfun.com/v1", Region::China, AdapterKind::OpenAI),
    preset("Yi (01.AI)", "https://api.lingyiwanwu.com/v1", Region::China, AdapterKind::OpenAI),
    preset("Baichuan", "https://api.baichuan-ai.com/v1", Region::China, AdapterKind::OpenAI),
    preset("Ollama", "http://localhost:11434/v1", Region::Local, AdapterKind::OpenAI),
    preset("LM Studio", "http://localhost:1234/v1", Region::Local, AdapterKind::OpenAI),
];

/// The preset with this address, or failing that, on the same host, like
/// Gemini's OpenAI-style address.
pub fn preset_for(base_url: &str) -> Option<&'static Preset> {
    let base = base_url.trim().trim_end_matches('/');
    PRESETS.iter().find(|preset| preset.base_url.eq_ignore_ascii_case(base)).or_else(|| {
        let host = host_of(base)?;
        PRESETS.iter().find(|preset| host_of(preset.base_url).as_deref() == Some(host.as_str()))
    })
}

/// The host and port of an address, in lower case.
fn host_of(base_url: &str) -> Option<String> {
    let rest = base_url.split_once("://")?.1;
    let host = rest.split(['/', '?']).next().filter(|host| !host.is_empty())?;
    Some(host.to_ascii_lowercase())
}

/// How to reach a server: the genai adapter for it, and its address.
fn route(base_url: &str) -> (AdapterKind, String) {
    let base = base_url.trim().trim_end_matches('/');
    match preset_for(base) {
        // Gemini's and Anthropic's own APIs have their own addresses, so an
        // address for an OpenAI-style one on the same host moves there.
        Some(preset) if !speaks_openai(preset.adapter) => (preset.adapter, preset.base_url.to_owned()),
        Some(preset) => (preset.adapter, base.to_owned()),
        None => (AdapterKind::OpenAI, base.to_owned()),
    }
}

fn speaks_openai(adapter: AdapterKind) -> bool {
    !matches!(adapter, AdapterKind::Gemini | AdapterKind::Anthropic | AdapterKind::MiniMax)
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
    /// An assistant turn as the provider sent it, sent back whole: its
    /// thinking and signatures go with it.
    pub native: Option<Value>,
}

impl Message {
    fn text(role: Role, text: impl Into<String>) -> Self {
        Self { role, parts: vec![Part::Text(text.into())], tool_calls: Vec::new(), tool_call_id: None, native: None }
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
        Self { role: Role::Assistant, parts, tool_calls: reply.tool_calls.clone(), tool_call_id: None, native: reply.native.clone() }
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
            native: None,
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

    /// What genai sends for a message that isn't a tool's result.
    fn content(&self) -> MessageContent {
        if let Some(kept) = self.native.as_ref().and_then(|native| serde_json::from_value::<MessageContent>(native.clone()).ok()) {
            return kept;
        }
        let mut parts: Vec<ContentPart> = self
            .parts
            .iter()
            .filter(|part| !matches!(part, Part::Text(text) if text.is_empty()))
            .map(|part| match part {
                Part::Text(text) => ContentPart::Text(text.clone()),
                Part::Image { media_type, data } => {
                    let encoded = base64::engine::general_purpose::STANDARD.encode(data);
                    ContentPart::Binary(Binary::from_base64(media_type.clone(), encoded, None))
                }
            })
            .collect();
        parts.extend(self.tool_calls.iter().map(|call| {
            ContentPart::ToolCall(genai_chat::ToolCall {
                call_id: call.id.clone(),
                fn_name: call.name.clone(),
                fn_arguments: match call.args() {
                    Value::Null => Value::Object(Default::default()),
                    args => args,
                },
                thought_signatures: None,
            })
        }));
        MessageContent::from_parts(parts)
    }
}

/// The messages as genai takes them. Results of tools called together go
/// back together, as Anthropic and Gemini want.
fn to_genai(messages: &[Message]) -> Vec<ChatMessage> {
    let mut sent = Vec::new();
    let mut results: Vec<ToolResponse> = Vec::new();
    for message in messages {
        if message.role == Role::Tool {
            results.push(ToolResponse::new(message.tool_call_id.clone().unwrap_or_default(), message.plain_text()));
            continue;
        }
        if !results.is_empty() {
            sent.push(ChatMessage::tool(MessageContent::from_tool_responses(std::mem::take(&mut results))));
        }
        let content = message.content();
        sent.push(match message.role {
            Role::System => ChatMessage::system(content),
            Role::Assistant => ChatMessage::assistant(content),
            Role::User | Role::Tool => ChatMessage::user(content),
        });
    }
    if !results.is_empty() {
        sent.push(ChatMessage::tool(MessageContent::from_tool_responses(results)));
    }
    sent
}

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
    /// How hard a thinking model thinks. None: the model's own default.
    pub effort: Option<Effort>,
    /// The start of the talk stays the same from turn to turn, so the
    /// server may keep it, where it can.
    pub cache: bool,
}

/// How hard a thinking model thinks before it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effort {
    Low,
    Medium,
    High,
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
    /// The server stopped it at its length limit, maybe mid-sentence. A
    /// thinking model can use the whole limit before it answers.
    pub cut_off: bool,
    /// The turn as the provider sent it, for `Message::from_reply`.
    pub native: Option<Value>,
}

/// A piece of an answer as it is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Live<'a> {
    Text(&'a str),
    /// Thinking, from a model that shows it.
    Thinking(&'a str),
    /// The name of a tool it is calling.
    Calling(&'a str),
}

/// Said to a model whose answer was cut off, so it carries on.
pub const GO_ON: &str = "Your last answer was cut off. Don't think out loud: take the next step with a tool now, and keep any words short.";

pub struct Llm {
    client: Option<Client>,
    adapter: AdapterKind,
    /// With a `/` at the end, as genai wants.
    endpoint: String,
    key: String,
    model: String,
}

impl Llm {
    pub fn new(config: &LlmConfig) -> Self {
        let (adapter, base) = route(&config.base_url);
        Self::with_adapter(config, adapter, &base)
    }

    fn with_adapter(config: &LlmConfig, adapter: AdapterKind, base: &str) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(180))
            .build()
            .unwrap_or_default();
        let client = Client::builder().with_reqwest(http).build();
        if let Err(err) = &client {
            tracing::warn!(%err, "could not set up the LLM client");
        }
        Self {
            client: client.ok(),
            adapter,
            endpoint: format!("{}/", base.trim_end_matches('/')),
            key: config.api_key.as_deref().map(str::trim).unwrap_or_default().to_owned(),
            model: config.model.trim().to_owned(),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    fn client(&self) -> Result<&Client, AiError> {
        self.client.as_ref().ok_or_else(|| AiError::answer(SERVICE, "the LLM client could not start"))
    }

    fn target(&self) -> ServiceTarget {
        ServiceTarget {
            endpoint: Endpoint::from_owned(self.endpoint.clone()),
            auth: AuthData::from_single(self.key.clone()),
            model: ModelIden::new(self.adapter, self.model.clone()),
        }
    }

    /// Asks for JSON, and asks again without when the server refuses it.
    /// Some refuse with an error that can't be read, so any refusal counts.
    fn json_refused(request: &ChatRequest, err: &AiError) -> bool {
        request.json && matches!(err, AiError::Status { status: 400 | 422, .. } | AiError::Answer { .. })
    }

    pub async fn chat(&self, request: &ChatRequest) -> Result<ChatReply, AiError> {
        match self.send(request, request.json).await {
            Err(err) if Self::json_refused(request, &err) => self.send(request, false).await,
            other => other,
        }
    }

    /// Like `chat`, giving `live` each piece of the answer as it comes.
    pub async fn chat_live(&self, request: &ChatRequest, mut live: impl FnMut(Live<'_>) + Send) -> Result<ChatReply, AiError> {
        match self.stream(request, request.json, &mut live).await {
            Err(err) if Self::json_refused(request, &err) => self.stream(request, false, &mut live).await,
            other => other,
        }
    }

    async fn stream(&self, request: &ChatRequest, json_mode: bool, live: &mut (impl FnMut(Live<'_>) + Send)) -> Result<ChatReply, AiError> {
        let (chat, options) = Self::prepare(request, json_mode);
        // The whole turn comes back at the end, signatures and all.
        let options = options.with_capture_content(true).with_capture_tool_calls(true).with_capture_reasoning_content(true).with_capture_usage(true);
        let mut events = match self.client()?.exec_chat_stream(self.target(), chat, Some(&options)).await {
            Ok(answer) => answer.stream,
            // A server that can't stream answers all at once.
            Err(genai::Error::AdapterNotSupported { .. }) => return self.send(request, json_mode).await,
            Err(err) => return Err(ai_error(err)),
        };
        while let Some(event) = events.next().await {
            match event.map_err(ai_error)? {
                ChatStreamEvent::Chunk(chunk) => live(Live::Text(&chunk.content)),
                ChatStreamEvent::ReasoningChunk(chunk) => live(Live::Thinking(&chunk.content)),
                ChatStreamEvent::ToolCallChunk(chunk) if !chunk.tool_call.fn_name.is_empty() => live(Live::Calling(&chunk.tool_call.fn_name)),
                ChatStreamEvent::End(end) => {
                    return Ok(reply(
                        end.captured_content.unwrap_or_default(),
                        end.captured_reasoning_content,
                        end.captured_stop_reason,
                        end.captured_usage.unwrap_or_default(),
                    ));
                }
                _ => {}
            }
        }
        Err(AiError::answer(SERVICE, "the answer stopped before it ended"))
    }

    fn prepare(request: &ChatRequest, json_mode: bool) -> (genai_chat::ChatRequest, ChatOptions) {
        let mut chat = genai_chat::ChatRequest::from_messages(to_genai(&request.messages));
        if !request.tools.is_empty() {
            chat = chat.with_tools(request.tools.iter().map(|tool| {
                genai_chat::Tool::new(tool.name.clone()).with_description(tool.description.clone()).with_schema(tool.parameters.clone())
            }));
        }
        let mut options = ChatOptions::default();
        if let Some(temperature) = request.temperature {
            options = options.with_temperature(f64::from(temperature));
        }
        if let Some(max_tokens) = request.max_tokens {
            options = options.with_max_tokens(max_tokens);
        }
        if json_mode {
            options = options.with_response_format(ChatResponseFormat::JsonMode);
        }
        if let Some(effort) = request.effort {
            options = options.with_reasoning_effort(match effort {
                Effort::Low => ReasoningEffort::Low,
                Effort::Medium => ReasoningEffort::Medium,
                Effort::High => ReasoningEffort::High,
            });
        }
        if request.cache {
            options = options.with_cache_control(CacheControl::Ephemeral);
        }
        (chat, options)
    }

    async fn send(&self, request: &ChatRequest, json_mode: bool) -> Result<ChatReply, AiError> {
        let (chat, options) = Self::prepare(request, json_mode);
        let answer = self.client()?.exec_chat(self.target(), chat, Some(&options)).await.map_err(ai_error)?;
        Ok(reply(answer.content, answer.reasoning_content, answer.stop_reason, answer.usage))
    }

    /// The models the server offers, for the settings.
    pub async fn models(&self) -> Result<Vec<String>, AiError> {
        let config = (Endpoint::from_owned(self.endpoint.clone()), AuthData::from_single(self.key.clone()));
        let mut names: Vec<String> = self
            .client()?
            .all_model_names(self.adapter, config)
            .await
            .map_err(ai_error)?
            .into_iter()
            .map(|name| name.strip_prefix("models/").unwrap_or(&name).to_owned())
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Checks the address, key and model with a tiny question.
    pub async fn test(&self) -> Result<String, AiError> {
        let request = ChatRequest {
            // No length limit: a thinking model can use a small one up before
            // it says a word.
            messages: vec![Message::user("Reply with the single word OK.")],
            ..ChatRequest::default()
        };
        let reply = self.chat(&request).await?;
        Ok(reply.text.trim().to_owned())
    }
}

fn reply(
    mut content: MessageContent,
    reasoning_content: Option<String>,
    stop_reason: Option<genai_chat::StopReason>,
    usage: genai_chat::Usage,
) -> ChatReply {
    // Results are matched to calls by id, so each call gets one.
    let calls = content.iter_mut().filter_map(|part| match part {
        ContentPart::ToolCall(call) => Some(call),
        _ => None,
    });
    for (ix, call) in calls.enumerate() {
        if call.call_id.trim().is_empty() {
            call.call_id = format!("call_{ix}");
        }
    }
    let tool_calls = content
        .tool_calls()
        .into_iter()
        .map(|call| ToolCall {
            id: call.call_id.clone(),
            name: call.fn_name.clone(),
            arguments: match &call.fn_arguments {
                Value::String(text) => text.clone(),
                Value::Null => "{}".to_owned(),
                other => other.to_string(),
            },
        })
        .collect();
    let text = content.joined_texts().unwrap_or_default();
    if !content.contains_reasoning_content()
        && let Some(reasoning) = reasoning_content
    {
        content.push(ContentPart::ReasoningContent(reasoning));
    }
    let tokens = |count: Option<i32>| count.and_then(|count| u64::try_from(count).ok()).unwrap_or(0);
    ChatReply {
        text,
        tool_calls,
        usage: Usage { input_tokens: tokens(usage.prompt_tokens), output_tokens: tokens(usage.completion_tokens) },
        cut_off: stop_reason.as_ref().is_some_and(genai_chat::StopReason::is_max_tokens),
        native: serde_json::to_value(&content).ok(),
    }
}

fn ai_error(err: genai::Error) -> AiError {
    use genai::webc::Error as Web;
    match err {
        genai::Error::WebModelCall { webc_error, .. } | genai::Error::WebAdapterCall { webc_error, .. } => match webc_error {
            Web::ResponseFailedStatus { status, body, .. } => status_error(status.as_u16(), &body),
            Web::Reqwest(err) => AiError::network(SERVICE, err),
            other => AiError::answer(SERVICE, excerpt(&other.to_string())),
        },
        genai::Error::HttpError { status, body, .. } => status_error(status.as_u16(), &body),
        // A streamed answer the server turned down holds the refusal inside.
        genai::Error::WebStream { error, cause, .. } => match error.downcast_ref::<genai::Error>() {
            Some(genai::Error::HttpError { status, body, .. }) => status_error(status.as_u16(), body),
            _ => AiError::answer(SERVICE, excerpt(&cause)),
        },
        genai::Error::ChatResponse { body, .. } => {
            AiError::answer(SERVICE, excerpt(&error_words(&body).unwrap_or_else(|| body.to_string())))
        }
        // Its own words, without the request it sent.
        genai::Error::ChatResponseGeneration { cause, .. } => AiError::answer(SERVICE, excerpt(&cause)),
        genai::Error::RequiresApiKey { .. } | genai::Error::NoAuthData { .. } => AiError::answer(SERVICE, "it needs an API key"),
        other => AiError::answer(SERVICE, excerpt(&other.to_string())),
    }
}

/// A refusal, in the server's own words when it sent JSON.
fn status_error(status: u16, body: &str) -> AiError {
    let words = serde_json::from_str::<Value>(body).ok().and_then(|value| error_words(&value)).unwrap_or_else(|| body.to_owned());
    AiError::Status { service: SERVICE, status, body: excerpt(&words) }
}

/// The words of an error a server sent: `{"error":{"message":…}}`,
/// Gemini's `[{"error":{…}}]`, `{"error":"…"}` or `{"message":…}`.
fn error_words(value: &Value) -> Option<String> {
    let error = value.get("error").or_else(|| value.get(0).and_then(|first| first.get("error"))).unwrap_or(value);
    match error {
        Value::String(text) => Some(text.clone()),
        _ => error.get("message").and_then(Value::as_str).map(str::to_owned),
    }
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

    /// The JSON body of a request the test server saw.
    fn body(seen: &str) -> Value {
        serde_json::from_str(seen.split("\r\n\r\n").nth(1).unwrap_or_default()).unwrap()
    }

    #[test]
    fn local_servers_are_known() {
        assert!(is_local("http://localhost:11434/v1"));
        assert!(is_local("http://127.0.0.1:1234/v1"));
        assert!(is_local("http://[::1]:8080/v1"));
        assert!(!is_local("https://api.openai.com/v1"));
        assert!(!is_local("https://localhost.example.test/v1"));
    }

    #[test]
    fn presets_are_found_by_address() {
        assert_eq!(preset_for("https://api.deepseek.com/v1/").map(|preset| preset.name), Some("DeepSeek"));
        assert!(!preset_for("http://localhost:11434/v1").unwrap().needs_key);
        assert_eq!(preset_for("http://localhost:1234/v1").map(|preset| preset.name), Some("LM Studio"));
        assert_eq!(preset_for("https://example.test/v1"), None);
        // Gemini's OpenAI-style address, from before, finds Gemini.
        assert_eq!(preset_for("https://generativelanguage.googleapis.com/v1beta/openai").map(|preset| preset.name), Some("Google Gemini"));
    }

    #[test]
    fn each_server_is_reached_its_own_way() {
        assert_eq!(route("https://generativelanguage.googleapis.com/v1beta/openai/"), (AdapterKind::Gemini, "https://generativelanguage.googleapis.com/v1beta".into()));
        assert_eq!(route("https://api.anthropic.com/v1"), (AdapterKind::Anthropic, "https://api.anthropic.com/v1".into()));
        assert_eq!(route("https://api.z.ai/api/coding/paas/v4"), (AdapterKind::Zai, "https://api.z.ai/api/coding/paas/v4".into()));
        assert_eq!(route("http://127.0.0.1:8080/v1/"), (AdapterKind::OpenAI, "http://127.0.0.1:8080/v1".into()));
    }

    #[test]
    fn json_is_found_in_chatty_answers() {
        assert_eq!(extract_json(r#"{"a":1}"#), Some(json!({"a":1})));
        assert_eq!(extract_json("Sure!\n```json\n{\"a\": 2}\n```"), Some(json!({"a":2})));
        assert_eq!(extract_json("Here: {\"a\": 3} hope it helps"), Some(json!({"a":3})));
        assert_eq!(extract_json("no json"), None);
    }

    #[test]
    fn errors_are_read_for_their_words() {
        let gemini = json!([{ "error": { "code": 400, "message": "Function call is missing a thought_signature.", "status": "INVALID_ARGUMENT" } }]);
        assert_eq!(error_words(&gemini).as_deref(), Some("Function call is missing a thought_signature."));
        assert_eq!(error_words(&json!({ "error": "busy" })).as_deref(), Some("busy"));
        assert_eq!(error_words(&json!({ "choices": [] })), None);
        assert_eq!(status_error(429, r#"{"error":{"message":"Slow down."}}"#).to_string(), "The LLM answered 429: Slow down.");
    }

    #[tokio::test]
    async fn chat_reads_text_and_tool_calls() {
        let answer = r#"{"choices":[{"message":{"role":"assistant","content":"Hi","tool_calls":[
            {"id":"call_1","type":"function","function":{"name":"run_command","arguments":"{\"command\":\"ls\"}"}}]},"finish_reason":"tool_calls"}],
            "usage":{"prompt_tokens":12,"completion_tokens":5}}"#;
        let (address, server) = serve(vec![(200, answer.into())]).await;
        let llm = Llm::new(&config(&address));
        let request = ChatRequest {
            messages: vec![Message::system("Be brief."), Message::user_with_png("What is on screen?", vec![1, 2, 3])],
            tools: vec![Tool { name: "run_command".into(), description: "Runs".into(), parameters: json!({"type":"object"}) }],
            ..ChatRequest::default()
        };
        let reply = llm.chat(&request).await.unwrap();
        assert_eq!(reply.text, "Hi");
        assert_eq!(reply.tool_calls[0].args()["command"], "ls");
        assert_eq!(reply.usage, Usage { input_tokens: 12, output_tokens: 5 });
        assert!(!reply.cut_off);
        let seen = server.await.unwrap();
        assert!(seen[0].starts_with("POST /v1/chat/completions"), "{}", seen[0]);
        assert!(seen[0].to_ascii_lowercase().contains("authorization: bearer sk-test"));
        let sent = body(&seen[0]);
        assert_eq!(sent["model"], "test-model");
        assert_eq!(sent["tools"][0]["function"]["name"], "run_command");
        assert!(sent.to_string().contains("data:image/png;base64,AQID"), "{sent}");
    }

    #[tokio::test]
    async fn answers_cut_off_at_the_limit_are_told_apart() {
        let answer = r#"{"choices":[{"message":{"content":"The critical things on a Mac"},"finish_reason":"length"}]}"#;
        let (address, _server) = serve(vec![(200, answer.into())]).await;
        let reply = Llm::new(&config(&address)).chat(&ChatRequest { messages: vec![Message::user("hi")], ..ChatRequest::default() }).await.unwrap();
        assert!(reply.cut_off);
    }

    #[tokio::test]
    async fn results_of_calls_made_together_go_back_together() {
        let calls = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
            {"id":"c1","type":"function","function":{"name":"list_hosts","arguments":"{}"}},
            {"id":"c2","type":"function","function":{"name":"list_snippets","arguments":"{}"}}]}}]}"#;
        let done = r#"{"choices":[{"message":{"content":"Done."}}]}"#;
        let (address, server) = serve(vec![(200, calls.into()), (200, done.into())]).await;
        let llm = Llm::new(&config(&address));
        let mut request = ChatRequest { messages: vec![Message::user("make a flow")], ..ChatRequest::default() };
        let reply = llm.chat(&request).await.unwrap();
        request.messages.push(Message::from_reply(&reply));
        request.messages.push(Message::tool_result("c1", "web-1"));
        request.messages.push(Message::tool_result("c2", "none"));
        assert_eq!(llm.chat(&request).await.unwrap().text, "Done.");
        let sent = body(&server.await.unwrap()[1]);
        let messages = sent["messages"].as_array().unwrap();
        assert_eq!(messages[1]["tool_calls"][1]["id"], "c2");
        let answers: Vec<&Value> = messages.iter().filter(|message| message["role"] == "tool").collect();
        assert_eq!(answers.iter().map(|answer| answer["tool_call_id"].as_str().unwrap()).collect::<Vec<_>>(), ["c1", "c2"]);
    }

    #[tokio::test]
    async fn gemini_thought_signatures_go_back_with_their_calls() {
        let signed = r#"{"candidates":[{"content":{"role":"model","parts":[
            {"functionCall":{"name":"list_hosts","args":{}},"thoughtSignature":"abc="}]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#;
        let done = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Done."}]},"finishReason":"STOP"}]}"#;
        let (address, server) = serve(vec![(200, signed.into()), (200, done.into())]).await;
        let gemini = LlmConfig { base_url: address.clone(), api_key: Some("g-test".into()), model: "gemini-test".into() };
        let llm = Llm::with_adapter(&gemini, AdapterKind::Gemini, &address);
        let mut request = ChatRequest { messages: vec![Message::user("make a flow")], ..ChatRequest::default() };
        let reply = llm.chat(&request).await.unwrap();
        assert_eq!(reply.tool_calls[0].name, "list_hosts");
        request.messages.push(Message::from_reply(&reply));
        request.messages.push(Message::tool_result(reply.tool_calls[0].id.clone(), "web-1"));
        assert_eq!(llm.chat(&request).await.unwrap().text, "Done.");
        let seen = server.await.unwrap();
        assert!(seen[1].contains(":generateContent"), "{}", seen[1]);
        assert!(body(&seen[1]).to_string().contains(r#""thoughtSignature":"abc=""#), "{}", seen[1]);
    }

    #[tokio::test]
    async fn answers_stream_in_and_come_back_whole() {
        let events = [
            r#"{"choices":[{"delta":{"role":"assistant","content":"Hel"}}]}"#,
            r#"{"choices":[{"delta":{"content":"lo"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c9","type":"function","function":{"name":"click","arguments":"{\"x\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"5}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":7,"completion_tokens":3}}"#,
        ];
        let mut answer: String = events.iter().map(|event| format!("data: {event}\n\n")).collect();
        answer.push_str("data: [DONE]\n\n");
        let (address, server) = serve(vec![(200, answer)]).await;
        let llm = Llm::new(&config(&address));
        let mut seen = Vec::new();
        let request = ChatRequest { messages: vec![Message::user("hi")], ..ChatRequest::default() };
        let reply = llm
            .chat_live(&request, |live| match live {
                Live::Text(text) => seen.push(text.to_owned()),
                Live::Calling(name) => seen.push(format!("[{name}]")),
                Live::Thinking(_) => {}
            })
            .await
            .unwrap();
        assert_eq!(seen.first().map(String::as_str), Some("Hel"));
        assert!(seen.contains(&"[click]".to_owned()), "{seen:?}");
        assert_eq!(reply.text, "Hello");
        assert_eq!(reply.tool_calls, vec![ToolCall { id: "c9".into(), name: "click".into(), arguments: "{\"x\":5}".into() }]);
        assert!(reply.native.is_some());
        assert!(body(&server.await.unwrap()[0])["stream"].as_bool().unwrap_or_default());
    }

    #[tokio::test]
    async fn gemini_signatures_survive_a_streamed_answer() {
        let signed = r#"data: {"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"list_hosts","args":{}},"thoughtSignature":"abc="}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}

"#;
        let done = r#"data: {"candidates":[{"content":{"role":"model","parts":[{"text":"Done."}]},"finishReason":"STOP"}]}

"#;
        let (address, server) = serve(vec![(200, signed.into()), (200, done.into())]).await;
        let gemini = LlmConfig { base_url: address.clone(), api_key: Some("g-test".into()), model: "gemini-test".into() };
        let llm = Llm::with_adapter(&gemini, AdapterKind::Gemini, &address);
        let mut request = ChatRequest { messages: vec![Message::user("make a flow")], ..ChatRequest::default() };
        let reply = llm.chat_live(&request, |_| {}).await.unwrap();
        assert_eq!(reply.tool_calls[0].name, "list_hosts");
        request.messages.push(Message::from_reply(&reply));
        request.messages.push(Message::tool_result(reply.tool_calls[0].id.clone(), "web-1"));
        assert_eq!(llm.chat_live(&request, |_| {}).await.unwrap().text, "Done.");
        let seen = server.await.unwrap();
        assert!(seen[1].contains(":streamGenerateContent"), "{}", seen[1]);
        assert!(body(&seen[1]).to_string().contains(r#""thoughtSignature":"abc=""#), "{}", seen[1]);
    }

    #[tokio::test]
    async fn a_refused_stream_says_why() {
        let (address, _server) = serve(vec![(400, r#"{"error":{"message":"The model is busy."}}"#.into())]).await;
        let err = Llm::new(&config(&address)).chat_live(&ChatRequest { messages: vec![Message::user("hi")], ..ChatRequest::default() }, |_| {}).await.unwrap_err();
        assert_eq!(err.to_string(), "The LLM answered 400: The model is busy.");
    }

    #[tokio::test]
    async fn models_are_listed_by_name() {
        let answer = r#"{"object":"list","data":[{"id":"models/gemini-2.5-flash"},{"id":"gemini-2.5-pro","object":"model"},{"id":"gemini-2.5-pro"}]}"#;
        let (address, server) = serve(vec![(200, answer.into())]).await;
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
        let request = ChatRequest { messages: vec![Message::system("Answer in JSON."), Message::user("json please")], json: true, ..ChatRequest::default() };
        let reply = llm.chat(&request).await.unwrap();
        assert_eq!(extract_json(&reply.text), Some(json!({"x":1})));
        let seen = server.await.unwrap();
        assert!(seen[0].contains("response_format"));
        assert!(!seen[1].contains("response_format"));
    }
}
