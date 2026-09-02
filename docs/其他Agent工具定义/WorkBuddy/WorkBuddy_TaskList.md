# TaskList 工具定义
```json
{
    "type": "function",
    "function": {
        "name": "TaskList",
        "description": "使用该工具列出任务清单内全部任务。

## 使用场景

- 查询可以开始执行的任务（状态：'pending'、无负责人、不存在阻塞依赖）
- 查看项目整体进展
- 寻找被阻塞、需要先解决前置依赖的任务
- 完成一项任务后，查看解除阻塞的新任务，或者认领下一个可执行任务
- 存在多个可执行任务时，**优先按照任务ID顺序处理（ID从小到大）**，靠前的任务通常为后续任务提供基础上下文

## 返回内容

返回每条任务的概要信息：
- **id**：任务唯一标识（配合 TaskGet、TaskUpdate 使用）
- **subject**：任务简短描述
- **status**：可选值 'pending'（待处理）、'in_progress'（进行中）、'completed'（已完成）
- **owner**：分配负责人则填写智能体ID，空闲任务为空
- **blockedBy**：尚未完成的前置任务ID列表；存在阻塞依赖的任务，必须等待前置任务完成后才能认领

如需查看完整信息（详细描述、备注），请携带任务ID调用 TaskGet。

## 团队任务协同规则

团队协作时，所有成员共用同一份任务清单。成员应当遵守以下规则：
1. 定期调用 TaskList，**尤其是完成每项任务之后**，查找可执行任务或者刚解除阻塞的任务
2. 通过 TaskUpdate 认领未分配、无阻塞的任务（将 `owner` 设置为自身名称）。**优先按照任务ID从小到大选择任务**
3. 发现新增待办工作时，使用 TaskCreate 创建新任务
4. 任务完成后通过 TaskUpdate 标记为已完成，随后调用 TaskList 寻找下一项工作
5. 通过查看任务清单状态与其他成员协同工作
6. 若所有可查看任务均被阻塞，通知团队负责人或协助解决阻塞任务
",
        "parameters": {
            "type": "object",
            "properties": {},
            "additionalProperties": false,
            "$schema": "http://json-schema.org/draft-07/schema#"
        },
        "strict": false
    }
}
```