# skill_manage 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "skill_manage",
    "description": "管理技能（创建、更新、删除）。技能是您的程序化记忆——针对重复出现的任务类型的可复用方法。新技能将保存至 ~/.hermes/skills/；现有技能可在其所在位置进行修改。

    支持的操作：create（完整的 SKILL.md + 可选的分类）、patch（old_string/new_string——修复时的首选）、edit（完整重写 SKILL.md——仅用于重大修改）、delete、write_file、remove_file。

    在执行删除操作时，如果您要将此技能的内容合并到另一个技能中，请传入 absorbed_into=<umbrella>；如果您是在没有转发目标的情况下修剪（清理）它，请传入 absorbed_into=\"\"。这使得管理员（curator）无需猜测即可区分“合并”与“修剪”，从而确保下游消费者（如引用了旧技能名称的定时任务等）得到正确更新。您在 absorbed_into 中指定的目标必须已存在——请先创建/修补伞形技能（umbrella），然后再执行删除。

    何时创建：复杂任务成功完成（5次以上调用）、克服了错误、用户纠正的方法奏效、发现了非平凡的工作流，或用户要求您记住某个操作流程。
    何时更新：指令过时/错误、遇到特定操作系统的失败、在使用中发现缺失的步骤或陷阱。如果您使用了某个技能并遇到了它未涵盖的问题，请立即对其进行修补。

    在困难/迭代性的任务完成后，主动提议将其保存为技能。对于简单的一次性任务则跳过。在创建/删除前需与用户确认。

    优秀的技能应包含：触发条件、带有确切命令的编号步骤、陷阱（pitfalls）部分、验证步骤。使用 skill_view() 查看格式示例。

    已固定（Pinned）的技能仅受删除保护——skill_manage(action='delete') 会拒绝操作，并提示用户使用 hermes curator unpin <name> 进行解绑。已固定的技能仍然允许执行修补和编辑，以便您在发现陷阱时继续改进它们；固定操作仅防止不可恢复的丢失。",
    "parameters": {
      "type": "object",
      "properties": {
      "action": {
      "type": "string",
      "enum": [
      "create",
      "patch",
      "edit",
      "delete",
      "write_file",
      "remove_file"
      ]
      ,
      "description": "要执行的操作。"
      }
      ,
      "name": {
      "type": "string",
      "description": "技能名称（小写字母，可使用连字符/下划线，最多 64 个字符）。对于 patch/edit/delete/write_file/remove_file 操作，必须匹配一个现有的技能。"
      }
      ,
      "content": {
      "type": "string",
      "description": "完整的 SKILL.md 内容（YAML 前置元数据 + Markdown 正文）。'create' 和 'edit' 操作时为必填项。对于 'edit'，请先使用 skill_view() 读取该技能，并提供完整的更新文本。"
      }
      ,
      "old_string": {
      "type": "string",
      "description": "要在文件中查找的文本（'patch' 操作时为必填项）。除非 replace_all=true，否则该文本必须是唯一的。请包含足够的周围上下文以确保唯一性。"
      }
      ,
      "new_string": {
      "type": "string",
      "description": "替换文本（'patch' 操作时为必填项）。可以传入空字符串以删除匹配到的文本。"
      }
      ,
      "replace_all": {
      "type": "boolean",
      "description": "用于 'patch' 操作：替换所有匹配项，而不要求唯一匹配（默认值：false）。"
      }
      ,
      "category": {
      "type": "string",
      "description": "用于组织技能的可选分类/领域（例如 'devops'、'data-science'、'mlops'）。会创建一个子目录进行分组。仅在 'create' 操作时使用。"
      }
      ,
      "file_path": {
      "type": "string",
      "description": "技能目录内辅助文件的路径。对于 'write_file'/'remove_file'：为必填项，且必须位于 references/、templates/、scripts/ 或 assets/ 目录下。对于 'patch'：为可选，若省略则默认为 SKILL.md。"
      }
      ,
      "file_content": {
      "type": "string",
      "description": "文件内容。'write_file' 操作时为必填项。"
      }
      ,
      "absorbed_into": {
      "type": "string",
      "description": "仅用于 'delete' 操作——声明意图，以便管理员无需猜测即可区分“合并”与“修剪”。当此技能的内容被合并到另一个技能时，传入伞形技能的名称（目标必须已存在）。当该技能确实已过时且被修剪（无转发目标）时，传入空字符串。为了向后兼容，删除时省略此参数也是允许的，但下游工具（例如定时任务中的技能引用重写）将不得不猜测您的意图。"
      }
      }
      ,
      "required": [
      "action",
      "name"
      ]
    }
  }
}
```