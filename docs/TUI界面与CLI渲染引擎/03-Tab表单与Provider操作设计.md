# Tab 表单与 Provider 操作设计

> 配套 `01-产品设计.md` §4 与 `02-技术设计.md` §5。

---

## 1. Tab 字段顺序（5 + 1）

固定顺序，不可配置：

| # | 字段 | 类型 | 默认值 | 校验 |
|---|------|------|--------|------|
| 1 | protocol | Choice{anthropic, openai} | anthropic | 必选 |
| 2 | provider_name | Text（不脱敏） | "" | 非空、≤64 |
| 3 | model_name | Text（不脱敏） | "" | 非空、≤64 |
| 4 | end_point | Text（不脱敏） | "" | 非空、必须是 http(s):// 开头 |
| 5 | api_key | Text（脱敏态显示 `****<末4>`，编辑态明文） | "" | 非空 |
| 6 | 确认 | Confirm{[确认], [取消]} | 焦点在 [确认] | — |

校验失败的字段 Tab 会标红（边框红色 + `! <msg>`），Enter 不响应、焦点留在该 Tab。

---

## 2. 按键表

### 浏览态（默认）

| 键 | 行为 |
|----|------|
| `←` | 上一个 Tab；Tab 1 时跳到最后一个 |
| `→` | 下一个 Tab；最后一个 Tab 时跳到 Tab 1 |
| `Tab` | **子屏内 Tab 与主屏斜杠命令 Tab 冲突；子屏忽略斜杠 Tab 语义，纯做 ←/→ 切换** |
| `Enter` | 进入当前 Tab 的"编辑态"（见 §3） |
| `Esc` | 退出子屏；如有未保存修改，屏底提示 `! 有未保存修改,按 Ctrl-C 强退`（按 Esc 不直接退出） |
| `Ctrl-C` | 强退子屏 |

### 编辑态（仅 Tab 2/3/4/5 文本）

| 键 | 行为 |
|----|------|
| `←` `→` | 在文本内左右移动光标 |
| `Home` / `End` | 跳到首尾 |
| `Backspace` / `Delete` | 删除字符 |
| 任意可打印字符 | 插入到光标位置 |
| `Enter` | 退出编辑态（保留修改），回到浏览态 |
| `Esc` | 退出编辑态（保留修改），回到浏览态；与 Enter 等价 |

### 编辑态（Tab 1 protocol）

| 键 | 行为 |
|----|------|
| `Enter` / `Space` | 切换 `anthropic ⇄ openai` |
| `←` / `→` | 同上 |
| `Esc` | 退出编辑态 |

### 编辑态（Tab 6 确认）

| 键 | 行为 |
|----|------|
| `←` / `→` | 切换 `[确认]` / `[取消]` |
| `Enter` | 触发当前按钮 |

---

## 3. ProviderList 屏细节

### 字段 Tab

5 个数据 Tab：id / protocol / provider_name / model_name / end_point / api_key。**只读**；按 Enter 不进入编辑态，按 `s` 把当前记录设为 use（提交回主屏）。

### 操作 Tab（第 6 行）

- `[Switch as active]`：把当前 cursor 记录设为 use。
- `[Delete]`：进入 ProviderDelPicker。
- `[Back]`：退回主屏。

### 记录切换

- `↓` / `n`：下一条（最后一条时环回）。
- `↑` / `p`：上一条（第 0 条时跳到最后）。
- 整个 Tab 表的值随 cursor 变化重绘。

---

## 4. ProviderDel 屏细节

### Picker

- 列表展示所有记录（同 list 屏内容，但只读）。
- `↑` / `↓`：移动 cursor。
- `Enter`：进 Confirm 屏。
- `Esc`：退到主屏。

### Confirm

- 展示待删记录的完整内容（含脱敏 key）。
- `←` / `→`：切换 `[确认删除]` / `[取消]`。
- `Enter`：
  - `[确认删除]` → `db.delete(id)`，Toast `✓ 已删除 id=…`，回到主屏。
  - `[取消]` → Pop 回 Picker。
- `Esc`：等价 `[取消]`。

---

## 5. 错误与提示

| 场景 | 显示位置 | 文案 |
|------|----------|------|
| 字段空 | 该 Tab 边框红 + 屏底 | `! provider_name 不能为空` |
| end_point 非法 | 屏底 | `! end_point 必须以 http:// 或 https:// 开头` |
| DB UNIQUE 冲突 | 屏底 | `! 已存在相同 (protocol, provider, model, end_point) 的记录` |
| DB 其它错误 | 屏底 | `! <err>` |
| 表单未提交退出 | 屏底（仅 Esc 时） | `! 有未保存修改,再按一次 Esc 强退` |

---

## 6. API Key 脱敏一致性

代码层抽取一个工具函数 `mask_key(s: &str) -> String`：

- `s.len() < 4` → `"****"`。
- 否则 `"****" + s.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>()`。

供 `print_record`、`ProviderList::render`、`ProviderForm::render` 共用。

---

## 7. 测试用例（手动 + 单元）

- 单元：`TabForm` 接收 `←` `→` `Enter` `Esc` `Backspace` 序列，最终 focus/value 正确。
- 单元：`ProviderForm::validate()` 对每个字段分别校验（空 / 非法 end_point / 非法 protocol）。
- 手动：
  1. 启动 laew → 单独输入 `/provider` → 进入 ProviderList 屏。
  2. `/provider add` → 5 Tab 可前后切换；进入 Tab 5 显示明文；退出后回 `****`。
  3. `/provider del` → Picker 选第 2 条 → Confirm 选 [取消] → 回 Picker。
  4. 再走一次选 [确认删除] → 回到主屏并 Toast。
  5. 主屏输入 `/exit` → 退出。