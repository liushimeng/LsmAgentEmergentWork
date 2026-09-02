# TaskGet 工具定义
```json
{
    "name": "TaskGet",
    "description": "使用本工具根据任务ID从任务列表中获取对应任务信息。

    ## 适用场景
    - 开始执行任务前，需要查看完整描述与相关背景信息
    - 了解任务依赖关系（当前任务阻塞哪些任务、被哪些任务阻塞）
    - 接收到任务后，获取完整需求内容

    ## 输出内容
    返回任务全部详情：
    - **subject**：任务标题
    - **description**：详细需求与背景说明
    - **status**：任务状态，可选值：待处理、进行中、已完成
    - **blocks**：需等待当前任务完成后才能执行的任务
    - **blockedBy**：必须先完成、当前任务才可开始的前置任务

    ## 使用提示
    - 获取任务信息后，先确认其前置任务列表为空，再开始执行。
    - 如需查看所有任务概览，可使用任务列表工具。",
    "input_schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "对象",
        "properties": {
            "taskId": {
                "description": "待查询任务的ID",
                "type": "字符串"
            }
        },
        "required": [
            "taskId"
        ],
        "additionalProperties": false
    }
}
```
