//! SSE (Server-Sent Events) 字节流 → 事件流的同步解析器,以及协议无关的
//! [`ParseSink`] 聚合器,把两协议(Anthropic / OpenAI)各自的 wire 增量事件
//! 翻译成 [`Completion`](super::Completion)。
//!
//! 设计动机:见 `docs/SSE流式响应解析/02-技术设计.md`。
//! - `SseStream` 负责"按行 / 按空行"切分,不关心协议。
//! - `DeltaEvent` 是协议无关的事件类型。
//! - `ParseSink` 维护 `text` / `tool_calls` / `usage` / `stop_reason`,
//!   在收到 `Stop` 后可 `finish()` 得到 `Completion`。
//!
//! ## 用法示例
//! ```ignore
//! let mut sse = SseStream::with_max_buffer(64 * 1024);
//! let mut sink = ParseSink::new();
//! while let Some(chunk) = resp.chunk().await? {
//!     for ev in sse.push(&chunk)? {
//!         parser.feed(&ev, &mut sink)?;
//!     }
//! }
//! if let Some(ev) = sse.finish()? { parser.feed(&ev, &mut sink)?; }
//! let completion = sink.finish()?;
//! ```

use std::collections::VecDeque;
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{AgentError, Result};
use crate::llm::{Completion, ToolCallReq, Usage};

/// 默认行缓冲上限:64 KiB。超过即报错,防止畸形包吃光内存。
pub const DEFAULT_MAX_BUFFER: usize = 64 * 1024;

/// SSE 事件:由零或多行 `field: value` 组成,以空行结束。
///
/// 参考 <https://html.spec.whatwg.org/multipage/server-sent-events.html>。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// `event:` 字段(可选);OpenAI 不发此字段,Anthropic 必须发。
    pub event: Option<String>,
    /// `data:` 行,多行用 `\n` 拼接。
    pub data: String,
    /// `id:` 字段(本期不解析,保留口)。
    pub id: Option<String>,
    /// `retry:` 字段(本期不解析,保留口)。
    pub retry: Option<u32>,
}

/// 字节流 → 事件流 的同步解析器。
///
/// **不是** `async`,便于嵌入 `reqwest` 的同步 `chunk()` 循环。
pub struct SseStream {
    buf: Vec<u8>,
    max_buf: usize,
    /// 当前正在攒的事件;遇到空行时吐出。
    pending: Option<SseEvent>,
}

impl SseStream {
    pub fn new() -> Self {
        Self::with_max_buffer(DEFAULT_MAX_BUFFER)
    }

    pub fn with_max_buffer(max: usize) -> Self {
        Self {
            buf: Vec::with_capacity(4096),
            max_buf: max,
            pending: None,
        }
    }

    /// 喂入一块字节;返回 0..N 个已闭合事件(以空行分界)。
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>> {
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > self.max_buf {
            return Err(AgentError::Llm(format!(
                "SSE 行缓冲超过 {} 字节上限",
                self.max_buf
            )));
        }
        let mut out = Vec::new();
        loop {
            // 按 \n 切行(\n 或 \r\n 都接受)
            match self.buf.iter().position(|&b| b == b'\n') {
                None => break,
                Some(idx) => {
                    // 提取本行(去掉 \n;若前一个字符是 \r 也去掉)
                    let end = if idx > 0 && self.buf[idx - 1] == b'\r' { idx - 1 } else { idx };
                    let line = self.buf[..end].to_vec();
                    // consume 包括 \n
                    self.buf.drain(..=idx);
                    self.handle_line(&line, &mut out)?;
                }
            }
        }
        Ok(out)
    }

    /// 流结束:把残留的最后一个事件(若非空)产出。
    pub fn finish(&mut self) -> Result<Option<SseEvent>> {
        if !self.buf.is_empty() {
            // 上游可能没发最后的 \n
            let line = std::mem::take(&mut self.buf);
            let mut out = Vec::new();
            self.handle_line(&line, &mut out)?;
            if let Some(p) = self.pending.take() {
                out.push(p);
            }
            return Ok(out.into_iter().next());
        }
        Ok(self.pending.take())
    }

    fn handle_line(&mut self, line: &[u8], out: &mut Vec<SseEvent>) -> Result<()> {
        // 空行 = 事件结束
        if line.is_empty() {
            if let Some(ev) = self.pending.take() {
                if !ev.data.is_empty() || ev.event.is_some() {
                    out.push(ev);
                }
            }
            return Ok(());
        }
        // 注释行(: 开头)
        if line[0] == b':' {
            return Ok(());
        }
        // 找到第一个 ':'
        let colon = line.iter().position(|&b| b == b':');
        let (field, value) = match colon {
            Some(i) => {
                let f = &line[..i];
                let v = if i + 1 < line.len() && line[i + 1] == b' ' {
                    &line[i + 2..]
                } else {
                    &line[i + 1..]
                };
                (f, v)
            }
            None => (line, &[][..]),
        };
        let field = std::str::from_utf8(field)
            .map_err(|e| AgentError::Llm(format!("SSE field 非 UTF-8: {e}")))?;
        let value = std::str::from_utf8(value)
            .map_err(|e| AgentError::Llm(format!("SSE value 非 UTF-8: {e}")))?;
        let ev = self.pending.get_or_insert_with(SseEvent::default);
        match field {
            "event" => ev.event = Some(value.to_string()),
            "data" => {
                if !ev.data.is_empty() {
                    ev.data.push('\n');
                }
                ev.data.push_str(value);
            }
            "id" => ev.id = Some(value.to_string()),
            "retry" => {
                if let Ok(n) = value.parse::<u32>() {
                    ev.retry = Some(n);
                }
            }
            _ => { /* 忽略未知字段 */ }
        }
        Ok(())
    }
}

impl Default for SseStream {
    fn default() -> Self {
        Self::new()
    }
}

/// 协议无关的事件:协议 parser 把 `SseEvent` 翻译成 `DeltaEvent`,
/// 再喂给 [`ParseSink`],由 sink 聚合出 [`Completion`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaEvent {
    /// 输入侧 usage(Anthropic message_start / OpenAI 尾部 usage chunk)。
    InputUsage {
        input_tokens: u32,
        cache_read: u32,
        cache_creation: u32,
    },
    /// 输出侧 usage(Anthropic message_delta / OpenAI 尾部 usage chunk)。
    OutputUsage {
        output_tokens: u32,
    },
    /// 文本增量。
    TextDelta(String),
    /// 工具调用开始(携带 id / name)。
    ToolCallStart { id: String, name: String },
    /// 工具调用的 JSON 参数增量(原始 partial 字符串)。
    ToolCallJsonDelta(String),
    /// 工具调用结束(完整 JSON 已就绪)。
    ToolCallEnd,
    /// 终止信号。
    Stop { stop_reason: Option<String> },
    /// 上游显式错误(目前只有 Anthropic 流)。
    /// `kind` 为上游 error type(如 `overloaded_error`),供重试层分类。
    Error { kind: String, message: String },
}

/// 聚合器:把 `DeltaEvent` 累积成 `Completion`。
#[derive(Debug, Default)]
pub struct ParseSink {
    text: String,
    /// 当前正在拼接 JSON 的 tool_calls;队尾即"最近一个"。
    in_flight: VecDeque<InFlightToolCall>,
    /// 已完成的 tool_calls,按接收顺序。
    tool_calls: Vec<ToolCallReq>,
    /// 输入用量(来自 message_start / 尾部 usage chunk)。
    usage: Usage,
    /// 终止原因(Anthropic stop_reason / OpenAI finish_reason)。
    stop_reason: Option<String>,
    /// 流是否已收到终止信号。
    finished: bool,
    /// 流式错误(若设置则 finish() 返回 Err)。(kind, message)
    errored: Option<(String, String)>,
}

#[derive(Debug, Clone)]
struct InFlightToolCall {
    id: String,
    name: String,
    json_buf: String,
}

impl ParseSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否已经收到 `Stop` 事件。
    pub fn finished(&self) -> bool {
        self.finished
    }

    pub fn usage(&self) -> Usage {
        self.usage
    }

    pub fn feed(&mut self, ev: DeltaEvent) -> Result<()> {
        match ev {
            DeltaEvent::InputUsage {
                input_tokens,
                cache_read,
                cache_creation,
            } => {
                // 覆盖式:Anthropic message_start 一次性给出;OpenAI usage chunk 覆盖
                self.usage.input_tokens = input_tokens;
                self.usage.cache_read_input_tokens = cache_read;
                self.usage.cache_creation_input_tokens = cache_creation;
            }
            DeltaEvent::OutputUsage { output_tokens } => {
                // 覆盖式:Anthropic message_delta / OpenAI 尾部 usage 都是累计值
                self.usage.output_tokens = output_tokens;
            }
            DeltaEvent::TextDelta(s) => self.text.push_str(&s),
            DeltaEvent::ToolCallStart { id, name } => {
                self.in_flight.push_back(InFlightToolCall {
                    id,
                    name,
                    json_buf: String::new(),
                });
            }
            DeltaEvent::ToolCallJsonDelta(s) => {
                if let Some(last) = self.in_flight.back_mut() {
                    last.json_buf.push_str(&s);
                }
            }
            DeltaEvent::ToolCallEnd => {
                if let Some(call) = self.in_flight.pop_back() {
                    // 三级回退:完整 JSON → partial JSON(恢复截断字段) → _raw 兜底。
                    let arguments: Value = serde_json::from_str(&call.json_buf)
                        .ok()
                        .or_else(|| {
                            // 长 tool_call 参数被 max_tokens / 中断截断时,恢复已完成的字段。
                            crate::agent::partial_json::parse_partial_json_object(&call.json_buf)
                        })
                        .unwrap_or_else(|| json!({ "_raw": call.json_buf }));
                    self.tool_calls.push(ToolCallReq {
                        id: call.id,
                        name: call.name,
                        arguments,
                    });
                }
            }
            DeltaEvent::Stop { stop_reason } => {
                if self.stop_reason.is_none() {
                    self.stop_reason = stop_reason;
                }
                self.finished = true;
            }
            DeltaEvent::Error { kind, message } => {
                self.errored = Some((kind, message));
            }
        }
        Ok(())
    }

    /// 终结:产生最终的 `Completion`。
    pub fn finish(self) -> Result<Completion> {
        if let Some((kind, message)) = self.errored {
            return Err(AgentError::LlmStream { kind, message });
        }
        // 若流式未显式终止(例如 [DONE] 之前的最后一个 chunk 之后没新数据),
        // 也把 in_flight 残留的 tool_calls 尝试 parse 出来,避免丢调用。
        let mut tool_calls = self.tool_calls;
        for call in self.in_flight {
            // 三级回退:完整 JSON → partial JSON(恢复截断字段) → _raw 兜底。
            let arguments: Value = serde_json::from_str(&call.json_buf)
                .ok()
                .or_else(|| crate::agent::partial_json::parse_partial_json_object(&call.json_buf))
                .unwrap_or_else(|| json!({ "_raw": call.json_buf }));
            tool_calls.push(ToolCallReq {
                id: call.id,
                name: call.name,
                arguments,
            });
        }
        Ok(Completion {
            text: self.text,
            tool_calls,
            usage: self.usage,
            stop_reason: self.stop_reason,
        })
    }
}

/// 流式读取响应体,带两级超时保护(P0:此前 laew 完全无超时,半开连接会永久卡死):
/// - `idle`:相邻两个 chunk 之间的最大间隔(超时 → 疑似半开连接,报可重试错误);
/// - `total`:单次尝试的整体上限(超时 → 无限慢流兜底)。
///
/// 每收到一段字节即调用 `on_chunk`;所有超时/传输错误统一映射为
/// [`AgentError::LlmNetwork`](可重试),由弹性层(`resilient.rs`)决定重试。
pub async fn stream_chunks(
    resp: reqwest::Response,
    idle: Duration,
    total: Duration,
    on_chunk: &mut (dyn FnMut(&[u8]) -> Result<()> + Send),
) -> Result<()> {
    let start = tokio::time::Instant::now();
    let mut resp = resp;
    loop {
        if start.elapsed() >= total {
            return Err(AgentError::LlmNetwork(format!(
                "SSE 总超时(超过 {total:?} 未完成):已放弃本次尝试"
            )));
        }
        match tokio::time::timeout(idle, resp.chunk()).await {
            Err(_) => {
                return Err(AgentError::LlmNetwork(format!(
                    "SSE 空闲超时({idle:?} 无新数据):疑似半开连接"
                )));
            }
            Ok(Err(e)) => {
                return Err(AgentError::LlmNetwork(format!("SSE 传输中断: {e}")));
            }
            Ok(Ok(None)) => break, // EOF
            Ok(Ok(Some(bytes))) => on_chunk(&bytes)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_simple_event() {
        let mut s = SseStream::new();
        let evs = s.push(b"event: ping\ndata: {\"ok\":true}\n\n").unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event.as_deref(), Some("ping"));
        assert_eq!(evs[0].data, "{\"ok\":true}");
    }

    #[test]
    fn sse_concatenates_multiple_data_lines() {
        let mut s = SseStream::new();
        let evs = s
            .push(b"data: line1\ndata: line2\ndata: line3\n\n")
            .unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "line1\nline2\nline3");
    }

    #[test]
    fn sse_ignores_comment_lines() {
        let mut s = SseStream::new();
        let evs = s
            .push(b": this is a comment\n: another\ndata: x\n\n")
            .unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "x");
    }

    #[test]
    fn sse_finishes_with_remaining_event() {
        let mut s = SseStream::new();
        s.push(b"event: ping\ndata: hello").unwrap();
        let ev = s.finish().unwrap().unwrap();
        assert_eq!(ev.event.as_deref(), Some("ping"));
        assert_eq!(ev.data, "hello");
    }

    #[test]
    fn sse_rejects_oversized_buffer() {
        let mut s = SseStream::with_max_buffer(8);
        let big = vec![b'x'; 64];
        let err = s.push(&big).unwrap_err();
        assert!(format!("{err}").contains("超过"));
    }

    #[test]
    fn sse_handles_carriage_return_line_endings() {
        let mut s = SseStream::new();
        let evs = s.push(b"data: hello\r\n\r\n").unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "hello");
    }

    #[test]
    fn sse_openai_style_no_event_field() {
        let mut s = SseStream::new();
        let evs = s
            .push(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n")
            .unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event, None);
        assert!(evs[0].data.contains("\"hi\""));
    }

    #[test]
    fn sink_text_delta_concat() {
        let mut sink = ParseSink::new();
        sink.feed(DeltaEvent::TextDelta("hello ".into())).unwrap();
        sink.feed(DeltaEvent::TextDelta("world".into())).unwrap();
        sink.feed(DeltaEvent::Stop { stop_reason: None }).unwrap();
        let c = sink.finish().unwrap();
        assert_eq!(c.text, "hello world");
        assert_eq!(c.stop_reason, None);
        assert_eq!(c.tool_calls.len(), 0);
    }

    #[test]
    fn sink_tool_call_assembles_json() {
        let mut sink = ParseSink::new();
        sink.feed(DeltaEvent::ToolCallStart { id: "c1".into(), name: "Bash".into() }).unwrap();
        sink.feed(DeltaEvent::ToolCallJsonDelta("{\"command\":".into())).unwrap();
        sink.feed(DeltaEvent::ToolCallJsonDelta("\"ls\"}".into())).unwrap();
        sink.feed(DeltaEvent::ToolCallEnd).unwrap();
        sink.feed(DeltaEvent::Stop { stop_reason: Some("tool_use".into()) }).unwrap();
        let c = sink.finish().unwrap();
        assert_eq!(c.tool_calls.len(), 1);
        assert_eq!(c.tool_calls[0].id, "c1");
        assert_eq!(c.tool_calls[0].name, "Bash");
        assert_eq!(c.tool_calls[0].arguments["command"], "ls");
        assert_eq!(c.stop_reason.as_deref(), Some("tool_use"));
    }

    #[test]
    fn sink_tool_call_bad_json_falls_back_to_raw() {
        let mut sink = ParseSink::new();
        sink.feed(DeltaEvent::ToolCallStart { id: "c1".into(), name: "X".into() }).unwrap();
        sink.feed(DeltaEvent::ToolCallJsonDelta("not json".into())).unwrap();
        sink.feed(DeltaEvent::ToolCallEnd).unwrap();
        sink.feed(DeltaEvent::Stop { stop_reason: None }).unwrap();
        let c = sink.finish().unwrap();
        assert_eq!(c.tool_calls[0].arguments["_raw"], "not json");
    }

    #[test]
    fn sink_input_usage_sets_input_tokens() {
        let mut sink = ParseSink::new();
        sink.feed(DeltaEvent::InputUsage { input_tokens: 100, cache_read: 50, cache_creation: 10 }).unwrap();
        sink.feed(DeltaEvent::Stop { stop_reason: None }).unwrap();
        let c = sink.finish().unwrap();
        assert_eq!(c.usage.input_tokens, 100);
        assert_eq!(c.usage.cache_read_input_tokens, 50);
        assert_eq!(c.usage.cache_creation_input_tokens, 10);
        assert_eq!(c.usage.output_tokens, 0);
    }

    #[test]
    fn sink_error_propagates() {
        let mut sink = ParseSink::new();
        sink.feed(DeltaEvent::Error {
            kind: "rate_limit_error".into(),
            message: "rate limited".into(),
        })
        .unwrap();
        let err = sink.finish().unwrap_err();
        assert!(format!("{err}").contains("rate limited"));
    }

    #[test]
    fn sink_completion_default_has_zero_usage() {
        let c = Completion::default();
        assert_eq!(c.usage.input_tokens, 0);
        assert_eq!(c.usage.output_tokens, 0);
        assert_eq!(c.usage.cache_read_input_tokens, 0);
    }

    // ========== partial JSON 截断恢复集成测试(L18) ==========

    #[test]
    fn sink_tool_call_truncated_json_recovers_partial() {
        let mut sink = ParseSink::new();
        sink.feed(DeltaEvent::ToolCallStart { id: "c1".into(), name: "Bash".into() }).unwrap();
        // 模拟截断:command 完整、timeout 完整、workdir 在字符串中间断流
        sink.feed(DeltaEvent::ToolCallJsonDelta("{\"command\":\"git log --oneline -n 50\",".into())).unwrap();
        sink.feed(DeltaEvent::ToolCallJsonDelta("\"timeout\":30,\"workdir\":\"/ho".into())).unwrap();
        sink.feed(DeltaEvent::ToolCallEnd).unwrap();
        sink.feed(DeltaEvent::Stop { stop_reason: Some("max_tokens".into()) }).unwrap();
        let c = sink.finish().unwrap();
        assert_eq!(c.tool_calls.len(), 1);
        let args = &c.tool_calls[0].arguments;
        let m = args.as_object().unwrap();
        assert_eq!(m["command"], "git log --oneline -n 50");
        assert_eq!(m["timeout"], 30);
        assert_eq!(m["workdir"], "/ho");
        assert_eq!(m[crate::agent::partial_json::TRUNCATED_KEY], true, "应标记截断");
    }

    #[test]
    fn sink_tool_call_unrecoverable_falls_back_to_raw() {
        let mut sink = ParseSink::new();
        sink.feed(DeltaEvent::ToolCallStart { id: "c1".into(), name: "X".into() }).unwrap();
        sink.feed(DeltaEvent::ToolCallJsonDelta("not json at all".into())).unwrap();
        sink.feed(DeltaEvent::ToolCallEnd).unwrap();
        sink.feed(DeltaEvent::Stop { stop_reason: None }).unwrap();
        let c = sink.finish().unwrap();
        assert_eq!(c.tool_calls.len(), 1);
        let m = c.tool_calls[0].arguments.as_object().unwrap();
        assert_eq!(m["_raw"], "not json at all");
    }

    #[test]
    fn sink_tool_call_complete_json_unchanged() {
        // 完整 JSON 不应被 partial 路径触碰,也不应标截断
        let mut sink = ParseSink::new();
        sink.feed(DeltaEvent::ToolCallStart { id: "c1".into(), name: "Bash".into() }).unwrap();
        sink.feed(DeltaEvent::ToolCallJsonDelta("{\"command\":\"ls\"}".into())).unwrap();
        sink.feed(DeltaEvent::ToolCallEnd).unwrap();
        sink.feed(DeltaEvent::Stop { stop_reason: Some("tool_use".into()) }).unwrap();
        let c = sink.finish().unwrap();
        let m = c.tool_calls[0].arguments.as_object().unwrap();
        assert_eq!(m["command"], "ls");
        assert!(m.get(crate::agent::partial_json::TRUNCATED_KEY).is_none());
    }
}
