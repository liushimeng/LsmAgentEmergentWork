#!/usr/bin/env python3
"""本地 Mock LLM 服务:模拟 OpenAI /chat/completions 与 Anthropic /v1/messages。

行为:第 1 次请求返回工具调用(Bash echo),第 2 次请求返回最终文本。
请求体落盘到 mock_requests.jsonl 供校验协议格式。
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

STATE = {}  # 按 path 分别计数,避免跨协议干扰
LOG_PATH = sys.argv[2] if len(sys.argv) > 2 else "mock_requests.jsonl"


def openai_tool_call(call_id, name, args):
    return {
        "id": call_id,
        "type": "function",
        "function": {"name": name, "arguments": json.dumps(args, ensure_ascii=False)},
    }


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):  # 安静模式
        pass

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(length) or b"{}")
        key = "oai" if "chat/completions" in self.path else "anth"
        n = STATE.get(key, 0) + 1
        STATE[key] = n
        with open(LOG_PATH, "a", encoding="utf-8") as f:
            f.write(json.dumps({"path": self.path, "call_no": n, "body": body}, ensure_ascii=False) + "\n")

        if key == "oai":
            if n == 1:
                msg = {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [openai_tool_call("call_mock_1", "Bash", {"command": "echo LAEW_MOCK_OK"})],
                }
            else:
                msg = {"role": "assistant", "content": "MOCK_FINAL_ANSWER: laew 端到端链路验证通过。"}
            resp = {"id": "mock", "object": "chat.completion", "choices": [{"index": 0, "message": msg, "finish_reason": "stop"}]}
        elif "v1/messages" in self.path:
            if n == 1:
                content = [{"type": "tool_use", "id": "toolu_mock_1", "name": "Bash", "input": {"command": "echo LAEW_ANTHROPIC_OK"}}]
                stop = "tool_use"
            else:
                content = [{"type": "text", "text": "MOCK_FINAL_ANSWER: laew Anthropic 链路验证通过。"}]
                stop = "end_turn"
            resp = {"id": "mock", "content": content, "stop_reason": stop, "role": "assistant"}
        else:
            self.send_response(404)
            self.end_headers()
            return

        data = json.dumps(resp, ensure_ascii=False).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8899
    print(f"mock llm server on 127.0.0.1:{port}, log -> {LOG_PATH}", flush=True)
    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
