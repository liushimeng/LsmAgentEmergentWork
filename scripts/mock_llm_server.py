#!/usr/bin/env python3
"""本地 Mock LLM 服务:模拟 OpenAI /chat/completions 与 Anthropic /v1/messages。

默认返回 SSE 流式响应(`text/event-stream`),
每个事件以 `\\n\\n` 分隔;可被 laew 的 SSE 解析器消费。

行为:第 1 次请求返回工具调用(Bash echo),第 2 次请求返回最终文本。
请求体落盘到 mock_requests.jsonl 供校验协议格式。
"""
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

STATE = {}  # 按 path 分别计数,避免跨协议干扰
LOG_PATH = sys.argv[2] if len(sys.argv) > 2 else "mock_requests.jsonl"


def openai_tool_call(call_id, name, args):
    return {
        "id": call_id,
        "type": "function",
        "function": {"name": name, "arguments": json.dumps(args, ensure_ascii=False)},
    }


def make_anthropic_sse(events):
    """events: list[dict],每个元素需含 'type' 字段;返回 SSE 字节串。"""
    parts = []
    for ev in events:
        parts.append(f"event: {ev['type']}\n".encode())
        data = json.dumps(ev.get("data", {}), ensure_ascii=False)
        parts.append(f"data: {data}\n\n".encode())
    return b"".join(parts)


def make_openai_sse(chunks, terminal_usage=None):
    """chunks: list[dict] 每条是一个普通 chat.completion.chunk;尾部 [DONE]。"""
    parts = []
    for ch in chunks:
        parts.append(f"data: {json.dumps(ch, ensure_ascii=False)}\n\n".encode())
    if terminal_usage is not None:
        parts.append(f"data: {json.dumps(terminal_usage, ensure_ascii=False)}\n\n".encode())
    parts.append(b"data: [DONE]\n\n")
    return b"".join(parts)


def build_anthropic_stream(call_no):
    """构造 Anthropic 一次完整流的 SSE 字节。"""
    if call_no == 1:
        # 第 1 次:返回工具调用
        events = [
            {
                "type": "message_start",
                "data": {
                    "type": "message_start",
                    "message": {
                        "id": "mock-msg-1",
                        "type": "message",
                        "role": "assistant",
                        "content": [],
                        "model": "mock-anthropic",
                        "stop_reason": None,
                        "usage": {"input_tokens": 13, "output_tokens": 1, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0},
                    },
                },
            },
            {
                "type": "content_block_start",
                "data": {
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "tool_use", "id": "toolu_mock_1", "name": "Bash", "input": {}},
                },
            },
            {
                "type": "content_block_delta",
                "data": {
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "input_json_delta", "partial_json": "{\"command\": \"echo LAEW_ANTHROPIC_OK\"}"},
                },
            },
            {
                "type": "content_block_stop",
                "data": {"type": "content_block_stop", "index": 0},
            },
            {
                "type": "message_delta",
                "data": {
                    "type": "message_delta",
                    "delta": {"stop_reason": "tool_use", "stop_sequence": None},
                    "usage": {"output_tokens": 23},
                },
            },
            {"type": "message_stop", "data": {"type": "message_stop"}},
        ]
    else:
        # 第 2 次:返回纯文本
        events = [
            {
                "type": "message_start",
                "data": {
                    "type": "message_start",
                    "message": {
                        "id": "mock-msg-2",
                        "type": "message",
                        "role": "assistant",
                        "content": [],
                        "model": "mock-anthropic",
                        "stop_reason": None,
                        "usage": {"input_tokens": 50, "output_tokens": 1, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0},
                    },
                },
            },
            {
                "type": "content_block_start",
                "data": {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
            },
            {
                "type": "content_block_delta",
                "data": {
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": "MOCK_FINAL_ANSWER: laew Anthropic 链路验证通过。"},
                },
            },
            {
                "type": "content_block_stop",
                "data": {"type": "content_block_stop", "index": 0},
            },
            {
                "type": "message_delta",
                "data": {
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                    "usage": {"output_tokens": 18},
                },
            },
            {"type": "message_stop", "data": {"type": "message_stop"}},
        ]
    return make_anthropic_sse(events)


def build_openai_stream(call_no):
    """构造 OpenAI 一次完整流的 SSE 字节。"""
    if call_no == 1:
        # 第 1 次:返回工具调用
        chunks = [
            {
                "id": "chatcmpl-mock-1",
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": "mock-openai",
                "choices": [
                    {"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": None}
                ],
            },
            {
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "tool_calls": [
                                {
                                    "index": 0,
                                    "id": "call_mock_1",
                                    "type": "function",
                                    "function": {"name": "Bash", "arguments": ""},
                                }
                            ]
                        },
                        "finish_reason": None,
                    }
                ]
            },
            {
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "tool_calls": [
                                {
                                    "index": 0,
                                    "function": {"arguments": "{\"command\": \"echo LAEW_MOCK_OK\"}"},
                                }
                            ]
                        },
                        "finish_reason": None,
                    }
                ]
            },
            {
                "choices": [
                    {"index": 0, "delta": {}, "finish_reason": "tool_calls"}
                ]
            },
        ]
        terminal = {
            "id": "chatcmpl-mock-1",
            "object": "chat.completion.chunk",
            "choices": [],
            "usage": {"prompt_tokens": 17, "completion_tokens": 25, "total_tokens": 42, "prompt_tokens_details": {"cached_tokens": 0}},
        }
    else:
        # 第 2 次:返回纯文本
        chunks = [
            {
                "id": "chatcmpl-mock-2",
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": "mock-openai",
                "choices": [
                    {"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": None}
                ],
            },
            {
                "choices": [
                    {"index": 0, "delta": {"content": "MOCK_FINAL_ANSWER: laew 端到端链路验证通过。"}, "finish_reason": None}
                ]
            },
            {
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
            },
        ]
        terminal = {
            "id": "chatcmpl-mock-2",
            "object": "chat.completion.chunk",
            "choices": [],
            "usage": {"prompt_tokens": 60, "completion_tokens": 20, "total_tokens": 80, "prompt_tokens_details": {"cached_tokens": 0}},
        }
    return make_openai_sse(chunks, terminal_usage=terminal)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):  # 安静模式
        pass

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(length) or b"{}")
        key = "oai" if "chat/completions" in self.path else "anth"
        n = STATE.get(key, 0) + 1
        STATE[key] = n
        # 记录关键请求头(User-Agent / Authorization / X-Session-Id / x-api-key / anthropic-version)
        headers = {
            "user-agent": self.headers.get("User-Agent", ""),
            "authorization": self.headers.get("Authorization", ""),
            "x-session-id": self.headers.get("X-Session-Id", ""),
            "x-api-key": self.headers.get("x-api-key", ""),
            "anthropic-version": self.headers.get("anthropic-version", ""),
            "accept": self.headers.get("Accept", ""),
        }
        with open(LOG_PATH, "a", encoding="utf-8") as f:
            f.write(json.dumps(
                {"path": self.path, "call_no": n, "headers": headers, "body": body},
                ensure_ascii=False,
            ) + "\n")

        if key == "oai":
            body_bytes = build_openai_stream(n)
        elif "v1/messages" in self.path:
            body_bytes = build_anthropic_stream(n)
        else:
            self.send_response(404)
            self.end_headers()
            return

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Content-Length", str(len(body_bytes)))
        self.end_headers()
        self.wfile.write(body_bytes)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8899
    print(f"mock llm server on 127.0.0.1:{port}, log -> {LOG_PATH}", flush=True)
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
