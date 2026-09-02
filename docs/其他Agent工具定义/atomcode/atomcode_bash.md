# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "bash",
    "description": "在工作目录中运行 shell 命令，并返回合并后的 stdout/stderr 及退出码。默认超时 60 秒（最长 300 秒）。破坏性命令（递归强制删除、sudo、dd、改写历史记录等）会被标记为高风险，可能需要审批。\n文件操作请优先使用专用工具而非 bash——它们能感知 gitignore、跨平台且更轻量：用 read_file 读取文件（不要用 cat/head/tail），用 grep 搜索文件内容（不要用 grep/rg），用 glob 按名称查找文件（不要用 find/fd），用 list_directory 查看目录树（不要用 ls），用 edit_file 修改文件、用 write_file 创建或覆盖文件。绝不要用 shell 命令编辑文件（sed/awk/perl -i，或 `>`/`>>`/tee 重定向）——这会破坏缩进和编码（尤其是在 Windows 上）并引发连锁问题；如果 edit_file 报告找不到你要替换的文本，请重新读取文件并复制确切文本，或用 write_file 重写整个文件——不要退回到 sed。请把 bash 留给真正的 shell 工作——git、构建、包管理器、运行命令——以及专用工具无法完成的管道/聚合操作（wc、sort、uniq、awk、git log）。",
    "parameters": {
      "type": "object",
      "properties": {
        "command": {
          "type": "string",
          "description": "要运行的 shell 命令"
        },
        "timeout": {
          "type": "integer",
          "description": "最长等待秒数（默认 60，最大 300）"
        }
      },
      "required": [
        "command"
      ]
    }
  }
}
```