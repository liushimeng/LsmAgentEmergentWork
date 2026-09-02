# TaskGet 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "TaskGet",
        "description": "使用该工具根据任务ID从任务列表中获取指定任务信息。

## 使用场景

- 准备开始执行任务前，需要查看完整描述与上下文信息
- 了解任务依赖关系（当前任务阻塞哪些任务、被哪些任务阻塞）
- 收到分配的任务后，获取完整需求

## 返回内容

返回任务全部详情：
- **subject**：任务标题
- **description**：详细需求与上下文
- **status**：状态，可选值 'pending'（待处理）、'in_progress'（进行中）、'completed'（已完成）
- **blocks**：需要等待当前任务完成才能执行的任务
- **blockedBy**：必须先行完成，当前任务才能启动的前置任务

## 使用提示

- 获取任务信息后，请确认 blockedBy 列表为空，再开始执行任务。
- 使用 TaskList 工具查看所有任务的概要信息。
",
        "parameters": {
            "type": "object",
            "properties": {
                "taskId": {
                    "type": "string",
                    "description": "待查询任务的ID"
                }
            },
            "required": [
                "taskId"
            ],
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#"
        },
        "strict": false
    }
}
```