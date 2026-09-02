# Agent 工具定义
```json
{
  "type": "function",
  "function": {
    "name": "team",
    "description": "运行并管理一个由专业化子代理组成的持久团队。使用 `delegate` 分派一个或多个独立任务，然后用返回的 run_id 配合 `status`、`wait` 或 `result` 查看进展或结果；用 `stop` 取消某次运行。角色决定了只读与限定范围写入的权限，以及快速模型与高能力模型的路由。worker 类角色必须提供非空的、相对于工作目录的写入范围（scope），且不能运行 Bash。",
    "parameters": {
      "type": "object",
      "oneOf": [
        {
          "properties": {
            "action": {
              "const": "delegate"
            },
            "tasks": {
              "type": "array",
              "minItems": 1,
              "items": {
                "type": "object",
                "properties": {
                  "description": {
                    "type": "string"
                  },
                  "prompt": {
                    "type": "string"
                  },
                  "role": {
                    "type": "string",
                    "enum": [
                      "planner",
                      "architect",
                      "explorer",
                      "implementer",
                      "rust",
                      "tui_ux",
                      "reviewer",
                      "tester",
                      "debugger",
                      "security",
                      "performance",
                      "docs_writer",
                      "release_manager",
                      "migration_compat"
                    ]
                  },
                  "scope": {
                    "type": "array",
                    "items": {
                      "type": "string"
                    },
                    "description": "worker 类角色必填；只读角色会忽略此参数。"
                  }
                },
                "required": [
                  "description",
                  "prompt",
                  "role"
                ]
              }
            }
          },
          "required": [
            "action",
            "tasks"
          ]
        },
        {
          "properties": {
            "action": {
              "const": "status"
            },
            "run_id": {
              "type": "string"
            }
          },
          "required": [
            "action"
          ]
        },
        {
          "properties": {
            "action": {
              "const": "wait"
            },
            "run_id": {
              "type": "string"
            },
            "timeout_secs": {
              "type": "integer",
              "minimum": 0,
              "maximum": 300
            }
          },
          "required": [
            "action",
            "run_id"
          ]
        },
        {
          "properties": {
            "action": {
              "const": "result"
            },
            "run_id": {
              "type": "string"
            }
          },
          "required": [
            "action",
            "run_id"
          ]
        },
        {
          "properties": {
            "action": {
              "const": "stop"
            },
            "run_id": {
              "type": "string"
            }
          },
          "required": [
            "action",
            "run_id"
          ]
        }
      ],
      "properties": {}
    }
  }
}
```