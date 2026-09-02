# execute_code 工具定义
```json
{
"type": "function",
"function": {
  "name": "execute_code",
  "description": "运行一个可以以编程方式调用 Hermes 工具的 Python 脚本。当您需要执行 3 次以上的工具调用并在它们之间包含处理逻辑、需要在大型工具输出进入您的上下文之前进行过滤/缩减、需要条件分支（如果 X 则 Y 否则 Z），或需要循环（获取 N 页、处理 N 个文件、失败时重试）时，请使用此功能。

  在以下情况请改用常规的工具调用：仅需单次工具调用且无需处理、您需要查看完整结果并进行复杂推理，或任务需要交互式用户输入。

  可通过 from hermes_tools import ... 使用以下工具：

  read_file(path: str, offset: int = 1, limit: int = 500) -> dict
  行号从 1 开始。返回 {content: ..., total_lines: N}
  write_file(path: str, content: str) -> dict
  始终覆盖整个文件。
  search_files(pattern: str, target=content, path=., file_glob=None, limit=50) -> dict
  target: content（在文件内容中搜索）或 files（按文件名查找文件）。返回 {matches: [...]}
  patch(path: str, old_string: str, new_string: str, replace_all: bool = False) -> dict
  将文件中的 old_string 替换为 new_string。
  terminal(command: str, timeout=None, workdir=None) -> dict
  仅限前台运行（不支持后台/伪终端）。返回 {output: ..., exit_code: N}

  限制：5 分钟超时，50KB 标准输出上限，每个脚本最多 50 次工具调用。terminal() 仅限前台运行（不支持后台或伪终端）。

  脚本在会话的工作目录中使用当前激活的虚拟环境（venv）的 Python 运行，因此项目依赖（如 pandas 等）和相对路径的工作方式与 terminal() 中相同。

  将最终结果打印（print）到标准输出（stdout）。在工具调用之间，请使用 Python 标准库（json、re、math、csv、datetime、collections 等）进行处理。

  此外，以下功能也可用（无需导入——已内置于 hermes_tools 中）：
  json_parse(text: str) — 带有 strict=False 参数的 json.loads；用于处理包含控制字符的 terminal() 输出
  shell_quote(s: str) — shlex.quote()；用于在将动态字符串插入 Shell 命令时进行转义
  retry(fn, max_attempts=3, delay=2) — 针对瞬时故障，以指数退避方式进行重试",
  "parameters": {
    "type": "object",
    "properties": {
    "code": {
    "type": "string",
    "description": "要执行的 Python 代码。使用 from hermes_tools import terminal, ... 导入工具，并将最终结果打印（print）到标准输出（stdout）。"
    }
    }
    ,
    "required": [
    "code"
    ]
  }
}
}
```