# patch 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "patch",
    "description": "在文件中进行精准的查找与替换编辑。在终端中请使用此功能代替 sed/awk。采用模糊匹配（9 种策略），因此轻微的空白/缩进差异不会导致操作失败。返回统一差异格式（unified diff）。编辑后会自动运行语法检查。

    替换模式（mode='replace'，默认）：查找一个唯一的字符串并进行替换。必填参数：mode, path, old_string, new_string。
    补丁模式（mode='patch'）：应用 V4A 格式的多文件补丁，用于批量更改。必填参数：mode, patch。",
    "parameters": {
      "type": "object",
      "properties": {
      "mode": {
        "type": "string",
        "enum": [
        "replace",
        "patch"
        ],
        "description": "编辑模式。'replace'（默认）：需要 path + old_string + new_string。'patch'：仅需补丁内容。",
        "default": "replace"
      },
      "path": {
      "type": "string",
      "description": "当 mode='replace' 时必填。要编辑的文件路径。"
      },
      "old_string": {
      "type": "string",
      "description": "当 mode='replace' 时必填。要查找并替换的确切文本。除非 replace_all=true，否则该文本在文件中必须是唯一的。请包含周围的上下文行以确保唯一性。"
      },
      "new_string": {
      "type": "string",
      "description": "当 mode='replace' 时必填。替换后的文本。传入空字符串 '' 可删除匹配到的文本。"
      },
      "replace_all": {
      "type": "boolean",
      "description": "替换所有匹配项，而不要求唯一匹配（默认：false）",
      "default": false
      },
      "patch": {
        "type": "string",
        "description": "当 mode='patch' 时必填。V4A 格式的补丁内容。格式：
        *** Begin Patch
        *** Update File: path/to/file
        @@ context hint @@
        context line
        -removed line
        +added line
        *** End Patch"
      },
      "cross_profile": {
      "type": "boolean",
      "description": "选择退出跨配置文件的软保护机制。默认为 false。【仅在】用户明确指示要编辑另一个 Hermes 配置文件的技能/插件/定时任务/记忆时，才设置为 true。",
      "default": false
      }
      },
      "required": [ "mode" ]
    }
  }
}
```