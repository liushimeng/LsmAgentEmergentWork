# MCP_Window_Use 第97轮:macos_legacy.rs 超线拆分 + 点击/拖拽复合行为测试覆盖

> 2026-09-20 第 97 轮。本文档承接第 96 轮中键修饰键补齐,完成 `macos_legacy.rs`
> (2032 行,超 1800 行红线)的目录化拆分,以及第 90 轮点击/拖拽复合行为的
> 单元测试覆盖补齐。
>
> 前置阅读:
> - `01-设计与解决方案.md`(第 84~88 轮,MCP_Window_Use 工具总体架构);
> - `02-鼠标键盘操控与优先级链方案.md`(第 90 轮,鼠标键盘原子能力与优先级链);
> - `04-鼠标键盘同时操作完善方案.md`(第 96 轮,中键修饰键补齐 + 本轮「拆分」立项);
> - 平台技术参考:`MCP_Window_Use_MacOS_技术文档.md`。

---

## 2. 设计总览

### 2.1 `macos_legacy.rs` 目录化拆分

按「单文件 ≤1800 行」规范,把单文件 2032 行拆为模块目录:

```
src/agent/window/macos_legacy/
  mod.rs            (~600 行)FFI 声明 + 平台常量 + MacOsDriver 结构 +
                          permission/ocr/screenshot 等 WindowDriver 入口
  ax_attrs.rs       (~300 行)AX 字面量 CFString 缓存 + kAX*Attribute/Action
                          inline getter + ax_error_text + MACOS_AX_UNAVAILABLE_HINT
                          + cfstr / cfnum_i64 / dict_get / ax_get / ax_get_string
  cg_event.rs       (~400 行)CGEvent 输入底座常量(K_CG_*) + key 键码/修饰键
                          解析 + cg_mod_flags / cg_mod_note + cg_move_cursor /
                          cg_scroll_lines / cg_send_key[_with_flags] / cg_click_at
                          / CgMouseButton / cg_click_at_ex / cg_drag / cg_type_text
  inspect.rs        (~600 行)inspect() + 浅树重试 + build_tree 递归 +
                          element_at_path / element_rect / element_action_names /
                          tree_is_shallow / is_frontmost_pid / window_element /
                          parse_window_id + merge_ax_actions / actions_for_role
  act.rs            (~430 行)act() match 分发(全 ControlAction 变体 → 优先级链)
                          + bring_to_front / is_frontmost / 公开 request_permission /
                          is_trusted / is_ax_trusted_with_retry
  tests.rs          (~150 行)原 shallow_tree_tests + key_combo_tests + 第 97 轮
                          新增点击/拖拽复合行为测试
```

**取舍**:

- **FFI 声明全部在 mod.rs**:extern "C" 块按 `#[link]` 分两组(ApplicationServices
  / CoreGraphics),常声明在 mod.rs 顶部,各子模块通过 `super::*` 取用;
  `ax_attrs.rs` 与 `cg_event.rs` 不重复声明 FFI,只引用 `super::*` 的常量与
  类型,与拆分前逐行等价。
- **`impl MacOsDriver` 跨文件拆分**:Rust 允许同一 struct 的 `impl` 块分布在
  同一 crate 任意文件,只要类型在相同 crate 路径可见。`inspect.rs` / `act.rs`
  / `mod.rs` 都用 `impl MacOsDriver`,与拆分前等价,LLM 视角零变化。
- **`list_windows_cg` 留在 mod.rs**:仅 ~50 行,且被 `macos_axui.rs` 通过
  `crate::agent::window::macos_legacy::list_windows_cg` 路径导入,留在 mod.rs
  可让 `pub use` 路径零变化,无需 re-export。
- **零外部行为变更**:拆分为纯目录化,代码逐行机械搬移零改写,所有函数签名
  / 私有项可见性标注 `pub(super)`,mod.rs 通过 `pub(super) use` 重新导出,
  子模块 `use super::*` 取用,与拆分前完全等价。
- **路径不变**:`crate::agent::window::macos_legacy::*` 路径下所有公开 API
  在拆分前后一致,外部调用点(`macos_axui.rs`、`window/mod.rs`、测试)零修改。

### 2.2 点击/拖拽复合行为单元测试覆盖

第 90 轮「鼠标键盘原子能力」上线时新增了 `ClickPoint` / `DoubleClickPoint` /
`RightClickPoint` / `MiddleClickPoint` / `DragPoint` 5 个变体,以及所有点击系
`modifiers` 修饰键。第 96 轮补齐 `MiddleClickPoint` 的 modifiers。本轮为
macOS 平台的「点击 + 修饰键 + 拖拽 + 修饰键」原语补单元测试覆盖,确保:

1. **多修饰键组合**:`parse_modifier_flags("ctrl+shift+alt")` / `cmd+shift` 等
   任意组合位或运算正确(此前仅验证两两组合);
2. **拖拽与点击的修饰键共用**:`parse_modifier_flags` 的输出 `u64` 位掩码
   既适用于 `cg_click_at_ex(..., flags)` 也适用于 `cg_drag(..., flags)`;
3. **修饰键别名前后兼容**:`ctrl` / `control` / `⌃`、`alt` / `option` / `opt` /
   `⌥`、`cmd` / `command` / `meta` / `⌘`、`shift` / `⇧` 别名映射到相同 flags 位;
4. **冒烟函数签名约束**:验证 `parse_ext` 在 `middle_click_point` / `drag_point`
   路径透传 `modifiers`(此前已有,本轮新增 `parse_ext` 拒绝以未知段为主键的
   modifiers 边界用例)。

新增测试约 4 组共 12 例,均在 `macos_legacy/tests.rs`。

---

## 3. 工具与链路层改动

| 文件 | 改动 | 行数 |
|------|------|------|
| `src/agent/window/macos_legacy.rs` | **删除**:整文件被目录化拆分 | -2032 |
| `src/agent/window/macos_legacy/mod.rs` | **新增**:FFI + Driver 入口 + list_windows_cg | +600 |
| `src/agent/window/macos_legacy/ax_attrs.rs` | **新增**:AX 字面量缓存 + AX helpers | +300 |
| `src/agent/window/macos_legacy/cg_event.rs` | **新增**:CGEvent 输入底座 | +400 |
| `src/agent/window/macos_legacy/inspect.rs` | **新增**:inspect + AX 树遍历 | +600 |
| `src/agent/window/macos_legacy/act.rs` | **新增**:act() 分发 + bring_to_front | +430 |
| `src/agent/window/macos_legacy/tests.rs` | **新增**:原 tests + 第 97 轮点击/拖拽测试 | +200 |
| `docs/MCP_Window_Use/05-macos_legacy超线拆分与点击拖拽复合行为测试覆盖方案.md` | **新增**:本轮设计 | +170 |
| `CLAUDE.md` | MCP_Window_Use 段落第 97 轮说明 | +15 |

---

## 4. 拆分原则与对账

1. **拆分前后 pub 路径零变化**:`crate::agent::window::macos_legacy::MacOsDriver`
   / `MACOS_AX_UNAVAILABLE_HINT` / `list_windows_cg` / `parse_modifier_flags`
   / `parse_key_combo` / `cg_mod_flags` / `cg_mod_note` 等全部可达;
2. **私有项可见性精确化**:跨子模块共享的私有函数/常量(`ax_error_text` /
   `parse_window_id` / `is_frontmost_pid`)在定义模块标 `pub(super)`,使用模块
   `use super::*`,与原单文件「私有」等价;
3. **测试搬入 `tests.rs`**:shallow_tree_tests / key_combo_tests 两个 mod 块
   逐字搬入;新增 `mod click_drag_combo_tests` 包含 4 组 12 例;
4. **FFI 常量集中**:ApplicationServices / CoreGraphics 链接块 + CGPoint /
   CGSize / K_CG_* / K_AX_ERROR_* 常量集中在 mod.rs 顶部,各子模块按需
   `use super::*`(无需重复声明)。

---

## 5. 验证方案

1. `cargo build` / `cargo test`:
   - 拆分前后 lib 测试**总数一致**(原 1197 + 本轮新增 12 = 1209);
   - `cargo test macos_legacy` 子模块测试 12 例全绿;
   - 编译期强约束:`macos_legacy/mod.rs` 显式 `pub use` 关键 API,
     `cargo build --release` 在 macOS 平台一次成功,Windows CI 无差异。
2. `bash testReport/run_e2e.sh`:全量回归通过(mock LLM,无需真实 Key)。
3. **零回归**:拆分前 `cargo build` 已有产物 + `cargo test` 结果,与拆分后
   完全等价;Windows / Linux CI 编译无任何变化。

---

## 6. 风险与取舍

- **拆分粒度**:6 个文件已能保证单文件 ≤1800 行红线;不再继续细分到
  `bring_to_front.rs` 等单函数文件,避免文件数量膨胀且每文件 < 100 行;
- **macos 编译验证**:本机为 Linux,macOS 编译验证依赖 CI;Linux 编译目标
  cfg(target_os = "macos") 跳过 macos_legacy 模块,本次拆分完全由 macOS CI 承担;
- **跨 impl 块分散**:`MacOsDriver` 的 `impl` 块在 mod.rs / inspect.rs / act.rs
  三处,Rust 编译器允许,但需保证**同名方法不重复定义**(本轮已逐一对账)。
