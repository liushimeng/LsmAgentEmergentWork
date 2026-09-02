# task 工具定义
```json
{
  "name": "task",
  "description": "启动一个新的 Agent 来自主处理复杂的、多步骤的任务。

  使用 Task 工具时，必须指定 subagent_type 参数以选择要使用的 Agent 类型。

  不应使用 Task 工具的情况：
  - 如果您想读取特定的文件路径，请使用 Read 或 Glob 工具，以更快地找到匹配项
  - 如果您正在搜索特定的类定义（如“class Foo”），请使用 Grep 工具，以更快地找到匹配项
  - 如果您在特定文件或 2-3 个文件集中搜索代码，请使用 Read 工具，以更快地找到匹配项
  - 如果没有合适的可用 Agent 来执行该任务，请直接使用其他工具

  使用说明：
  1. 只要可能，应同时启动多个 Agent 以最大限度地提高性能；为此，请在单个消息中使用多个工具调用
  2. 一旦您将工作委派给 Agent，就不要自己重复该工作。请继续处理不重叠的任务，或等待结果。对于后台任务，结果准备就绪时您将自动收到通知。
  3. Agent 完成后，它将向您返回一条消息。Agent 返回的结果对用户不可见。要向用户展示结果，您应该向用户发送一条文本消息，简明扼要地总结结果。输出中包含一个 task_id，您可以在以后重用它来继续同一个子 Agent 会话。
  4. 每次 Agent 调用都从一个全新的上下文开始，除非您提供 task_id 以恢复同一个子 Agent 会话（这将使用其以前的消息和工具输出继续）。当重新开始时，您的提示词应包含高度详细的任务描述，以便 Agent 自主执行，并且您应确切说明希望 Agent 在其返回给您的最终且唯一的消息中返回哪些信息。
  5. 通常应信任 Agent 的输出
  6. 明确告诉 Agent 您期望它是编写代码还是仅进行研究（搜索、文件读取、网页获取等），因为它不知道用户的意图。如果可能，请告诉它如何验证其工作（例如，相关的测试命令）。
  7. 如果 Agent 描述中提到应主动使用它，那么您应尽力在用户要求之前使用它。请自行判断。

  可用的 Agent 类型及其可访问的工具：
  - explore：专为快速探索代码库而设计的快速 Agent。当您需要通过模式快速查找文件（例如“src/components/**/*.tsx”）、搜索代码关键字（例如“API endpoints”）或回答有关代码库的问题（例如“API endpoints 如何工作？”）时，请使用此 Agent。调用此 Agent 时，请指定所需的彻底程度级别：“quick”表示基本搜索，“medium”表示适度探索，“very thorough”表示跨多个位置和命名约定的全面分析。
  - general：用于研究复杂问题和执行多步骤任务的通用 Agent。使用此 Agent 并行执行多个工作单元。",
  "parameters": {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": {
    "description": {
    "type": "string",
    "description": "任务的简短（3-5 个词）描述"
    },
    "prompt": {
    "type": "string",
    "description": "Agent 要执行的任务"
    },
    "subagent_type": {
    "type": "string",
    "description": "用于此任务的专用 Agent 类型"
    },
    "task_id": {
    "type": "string",
    "description": "仅在您打算恢复先前的任务时设置此字段（您可以传递先前的 task_id，任务将像之前一样继续同一个子 Agent 会话，而不是创建一个新的）"
    },
    "command": {
    "type": "string",
    "description": "触发此任务的命令"
    }
    },
    "required": [
    "description",
    "prompt",
    "subagent_type"
    ]
  }
}
```