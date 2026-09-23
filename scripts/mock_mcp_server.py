#!/usr/bin/env python3
"""mock_mcp_server.py: 最小 MCP stdio server(NDJSON 行帧),e2e/人工验证用。

实现 MCP 客户端最小闭环所需方法:
- initialize / notifications/initialized
- tools/list(两个工具:echo / fail,echo 正常返回,fail 返回 isError=true)
- tools/call
- resources/list / resources/read(mock://greeting 资源)

协议约束:每条 JSON-RPC 消息一行(NDJSON),消息内不含裸换行 —— 与
`src/mcp/transport.rs::StdioTransport` 对齐。设计见 docs/MCP_Use/01-设计与解决方案.md。

用法:python3 scripts/mock_mcp_server.py  (stdin/stdout 挂管道即可)
"""
import json
import sys


def reply(id_, result=None, error=None):
    frame = {"jsonrpc": "2.0", "id": id_}
    if error is not None:
        frame["error"] = error
    else:
        frame["result"] = result
    sys.stdout.write(json.dumps(frame, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def handle(msg):
    method = msg.get("method", "")
    id_ = msg.get("id")
    params = msg.get("params") or {}

    # 通知(无 id):只收不回。
    if id_ is None:
        return

    if method == "initialize":
        reply(id_, {
            "protocolVersion": params.get("protocolVersion", "2025-03-26"),
            "capabilities": {"tools": {}, "resources": {}},
            "serverInfo": {"name": "mock-mcp-server", "version": "0.1.0"},
            "instructions": "mock server for laew e2e",
        })
    elif method == "tools/list":
        reply(id_, {"tools": [
            {
                "name": "echo",
                "description": "回显 text 参数(连通性验证)",
                "inputSchema": {
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"],
                },
                "annotations": {"readOnlyHint": True},
            },
            {
                "name": "fail",
                "description": "固定返回 isError=true(错误路径验证)",
                "inputSchema": {"type": "object", "properties": {}},
            },
        ]})
    elif method == "tools/call":
        name = params.get("name", "")
        args = params.get("arguments") or {}
        if name == "echo":
            reply(id_, {
                "content": [{"type": "text", "text": "echo:" + str(args.get("text", ""))}],
                "isError": False,
            })
        elif name == "fail":
            reply(id_, {
                "content": [{"type": "text", "text": "mock tool failure"}],
                "isError": True,
            })
        else:
            reply(id_, error={"code": -32602, "message": "unknown tool: " + name})
    elif method == "resources/list":
        reply(id_, {"resources": [
            {
                "uri": "mock://greeting",
                "name": "greeting",
                "description": "mock 只读资源",
                "mimeType": "text/plain",
            },
        ]})
    elif method == "resources/read":
        uri = params.get("uri", "")
        if uri == "mock://greeting":
            reply(id_, {"contents": [
                {"uri": "mock://greeting", "mimeType": "text/plain", "text": "hello from mock resource"},
            ]})
        else:
            reply(id_, error={"code": -32002, "message": "resource not found: " + uri})
    else:
        reply(id_, error={"code": -32601, "message": "method not found: " + method})


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        handle(msg)


if __name__ == "__main__":
    main()
