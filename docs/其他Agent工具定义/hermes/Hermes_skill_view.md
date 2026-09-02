# skill_view 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "skill_view",
    "description": "技能（Skills）允许加载关于特定任务和工作流的信息，以及相关的脚本和模板。您可以加载某个技能的完整内容，或访问其关联的文件（参考资料、模板、脚本）。首次调用将返回 SKILL.md 的内容，以及一个显示可用参考资料/模板/脚本的 'linked_files' 字典。若要访问这些关联文件，请再次调用并传入 file_path 参数。",
    "parameters": {
      "type": "object",
      "properties": {
      "name": {
      "type": "string",
      "description": "技能名称（使用 skills_list 可查看可用的技能）。对于由插件提供的技能，请使用限定格式 'plugin:skill'（例如 'superpowers:writing-plans'）。"
      }
      ,
      "file_path": {
      "type": "string",
      "description": "可选：技能内关联文件的路径（例如 'references/api.md'、'templates/config.yaml'、'scripts/validate.py'）。省略此参数以获取主要的 SKILL.md 内容。"
      }
      }
      ,
      "required": [
      "name"
      ]
    }
  }
}
```