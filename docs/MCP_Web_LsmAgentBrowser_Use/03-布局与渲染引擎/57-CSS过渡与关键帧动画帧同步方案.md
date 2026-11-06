# 57-CSS 过渡与 @keyframes 动画帧同步方案

## 背景

40 号控件矩阵中「脚本动态生成控件」的三条渲染设计要点,前两条(重排重绘同步、组件样式隔离)中
Shadow DOM 为独立轮次目标,而**动画帧同步**一直缺位:

- `@keyframes` 在样式解析层被整段跳过(`style/mod.rs` 的 at-rule 分支);
- `transition`/`animation` 简写只透传进 `ComputedStyle.extra`,布局/渲染不可见;
- `requestAnimationFrame` 是 `setTimeout(fn, 16)` 的近似(JS 定时器语义本轮不动,见边界)。

结果:任何依赖 CSS 过渡/动画的站点(悬停反馈、轮播、加载动效、抽屉/弹层出入场)在 LsmAgentBrowser
里全部静止或瞬间跳变,截图与人工查看器都看不到中间态。

本轮落地「样式解析 → 动画引擎 → 布局钩子 → 几何/像素/JS getComputedStyle 同源 → 帧同步 ticker」
的完整垂直闭环。

## 状态口径

沿用 40 号文档:✅ 已实现 / 🆕 本轮新增 / 🟡 部分实现 / ⛔ 非目标。

## 一、核心落点(本轮代码)

| 能力 | 落点 |
| --- | --- |
| 简写/longhand 解析 | `src/agent/browser/style/anim_style.rs`(`AnimDecl`/`AnimSpec`/`TransitionSpec`/`Easing`) |
| `@keyframes` 收集 | `style/mod.rs::parse_block`(`keyframes`/`-webkit-keyframes` → `StyleEngine.keyframes`) |
| 动画引擎 | `src/agent/browser/anim.rs`(`AnimEngine`/`AnimValue`/`lerp_value`,内部可变,布局钩子只借 `&self`) |
| 布局钩子(唯一) | `layout/mod.rs::LayoutCtx::style_for` → `AnimEngine::track_and_apply`(diff 触发过渡 → 当前时刻值覆盖计算样式) |
| 布局入口 | `layout::compute_full_with_anim`(旧 `compute_full` 传一次性引擎,测试零改动) |
| 重排守卫 | `mod.rs::ensure_layout_locked`:有活跃动画时 html 未变也重排(隐藏模式按当前时钟取值) |
| 动作重排同源 | `api.rs::relayout_and_push` 等 4 处交互/视口重排改走 `tab.anim`(旧一次性引擎会让过渡滞后一拍才启动) |
| JS 同源 | `anim.rs::override_snapshot` + `document.rs::snapshot_nodes_with_rects(…, anim_ov)`:`getComputedStyle` 读到动画值 |
| 帧同步 | `anim.rs::ensure_anim_ticker`/`anim_tick`:查看器存活且动画变化中 40ms 节拍重排+推帧(仅跟随页,`ViewFollow` 语义),终帧/暂停 500ms 低频;hidden 零开销 |

运行时状态全在 `TabState.anim`(跨布局存活,导航整体换新);不回写 `tab.html`。

## 二、能力矩阵

### 1. 声明解析

| 能力 | 状态 | 说明 |
| --- | --- | --- |
| `animation` 简写 | 🆕 | 无序 token 流按类型识别(name/时间/缓动/迭代/方向/fill/play-state);逗号多动画列表,上限 8 条 |
| `transition` 简写 | 🆕 | 属性 + 时间 + 缓动 + delay;逗号多段;`property: all` 支持 |
| longhand 全集 | 🆕 | `animation-*` 8 项 + `transition-*` 4 项;逗号列表按位置回写,单项铺满多条目 |
| `@keyframes` | 🆕 | `from`/`to`/`N%` 步进、同一步进多块合并、步进排序;同名后声明覆盖 |
| timing function | 🆕 | `linear`/`ease`/`ease-in`/`ease-out`/`ease-in-out`/`cubic-bezier`(二分求解);`step-start/end` 近似离散 |
| 全局关键字 | 🆕 | `initial`/`unset`/`revert` 重置对应列表 |

### 2. 时间轴语义

| 能力 | 状态 | 说明 |
| --- | --- | --- |
| 关键帧插值 | 🆕 | 端点缺失用元素底层值补齐;段间缓动用动画级 easing |
| 迭代/方向 | 🆕 | `iteration-count`(数值/`infinite`)、`normal/reverse/alternate/alternate-reverse` 折返映射 |
| fill-mode | 🆕 | `none/forwards/backwards/both`;forwards/both 终帧保留到名字变化/节点 GC |
| delay | 🆕 | delay 内按 fill-backwards 显首帧,否则底层值 |
| play-state | 🆕 | `paused` 冻结已消耗时长,`running` 恢复平移 `start_ms` |
| 过渡触发 | 🆕 | 相邻布局的计算值 diff(transition-property 命中且 duration>0);起点=旧值、终点=新值 |
| 过渡重定向 | 🆕 | 运行中目标再变:从当前插值位置 retarget(与浏览器一致) |
| 叠加合成 | 🟡 | 过渡先铺、关键帧动画后盖(同 CSS 分层);同类多动画**后者胜**,无 additive/accumulate |

### 3. 可动画属性(第 57 轮支持集)

`opacity`、`transform`、`color`、`background-color`、`width`、`height`、`top/right/bottom/left`、
`margin-*`、`padding-*`(共 18 项)。其余属性不跟踪(离散跳变或不动)。

- `transform` 在**操作序列分量级**插值:translate/scale/rotate/skew/matrix 族内线性;
  族不匹配或序列长度不一致 → 50% 处离散跳变。百分比 translate 保留 `Length::Pct` 插值,布局期按盒求解。
- 颜色插值走**预乘 alpha**(transparent↔彩色中间帧不发黑)。
- `auto`/百分比宽高与 `margin:auto` 不可插值(离散);动画值写回时清除对应百分比/auto 标记。

### 4. 帧同步与同源

| 路径 | 行为 | 状态 |
| --- | --- | --- |
| 查看器(有头) | 动画变化中 40ms 强制重排+推**跟随页**帧;终帧/暂停/无动画 500ms 低频;后台页动画不劫持人工视口 | 🆕 |
| 截图/inspect/eval(含 hidden) | `ensure_layout_locked` 活跃动画时无条件重排,帧值=当前墙钟时刻的补间值 | 🆕 |
| JS `getComputedStyle` | 快照路径套 `override_snapshot`(与布局同一时刻口径;管线期间按入口时刻冻结) | 🆕 |
| `rect_map`/几何 | 动画值经计算样式流入布局,`getBoundingClientRect` 等自动跟随 | 🆕 |

## 三、引擎机制要点

1. **节点键** = `Document::stable_selector`(与 `form_values` 同族);仅对有 trackable 声明的节点
   惰性求键,零声明节点零开销。
2. **两本时间账**:`has_active`(含 fill 终帧与待落终值的过渡)驱动重排守卫;
   `is_animating`(仅变化中,排除终帧/暂停)驱动 ticker 节拍——终帧动画不烧 CPU。
3. **GC**:每布局回合 `begin_pass/end_pass` 记账,连续两回合未见的节点回收(防 display:none
   子树误杀);`track_and_apply` 在落终值的那一次调用回收结束条目。
4. **时钟**:`AnimEngine::now()` 默认墙钟(`now_ms`),`set_clock_override` 供测试注入,
   单测/布局测试全程确定性。
5. **幂等**:同一布局内 measure/final 多次调用 `style_for` 幂等(值未变不重复触发过渡/重启动画)。

## 四、关键根因记录(本轮实测)

1. **`has_active` 漏掉「已到时但未落终值」的过渡** → 过渡跑到 220/300 后 html 不再变,
   重排守卫跳过,画面冻结在结束前最后补间值。修:活跃判据按「运行表非空」,
   落终值那次调用回收后才归零。
2. **动作驱动重排用一次性引擎**(`relayout_and_push` 直调 `compute_full`)→ 过渡基线看不到,
   过渡要到下一次 inspect 才启动(e2e 实测滞后一拍)。修:所有交互/视口重排统一走 `tab.anim`。
3. **`@keyframes` 判据笔误**(`ends_with("keyframes")` 匹配不到 `keyframes name`)→ 关键帧表恒空,
   动画不启动。修:`starts_with` + `-webkit-` 前缀分支。

## 五、边界(如实上报,非 bug)

- `animationend`/`transitionend`/`animationstart` 等 DOM 事件**不派发**;
- transform 不做矩阵分解插值(rotate 跨 180° 的系数线性会收缩过中点);
- 关键帧步进内的 `animation-timing-function` 忽略(段间统一用动画级缓动);
- 多动画同属性叠加后者胜(无 additive/accumulate/composition);
- `requestAnimationFrame` 仍是 16ms 定时器近似(站点启动链敏感,不改语义);JS 驱动的逐帧
  变化经 `apply_js_reply` 的「动作批量→一次重排」保持帧对齐;
- 脚本管线多轮之间 `getComputedStyle` 的动画值按管线入口时刻冻结;
- DOM 结构变化(`html_rev` 漂移)导致稳定键错位时动画自然重启/丢失;
- `steps()` 多段/`frames()`/`animation-composition`/`will-change`/滚动驱动动画未实现。

## 六、测试矩阵

| 层 | 文件 | 覆盖 |
| --- | --- | --- |
| 解析 | `style/anim_style.rs::tests` | 简写无序 token、cubic-bezier、逗号列表、longhand 铺满、`@keyframes` 步进合并、trackable 门槛 |
| 引擎 | `browser/anim.rs::tests` | 插值(opacity/预乘色/transform 族)、fill/方向/迭代折返、暂停冻结、缺端点底层值补齐、过渡触发/retarget、只读覆盖 |
| 布局 | `layout/anim_tests.rs` | 关键帧宽度进几何、透明度进显示列表、跨布局过渡补间、transform 仿射进显示项、零声明零扰动 |
| 端到端 | `tests/web_anim_smoke.rs` | 真时钟短过渡:eval 改 style → 半程几何在起终点之间、`getComputedStyle.opacity` 中段、终值 300 |

验证命令:

```bash
cargo test --lib
cargo test --test web_anim_smoke
cargo test --no-fail-fast
./rebuild_restart_app.sh
```

## 七、后续路线

1. Shadow DOM / Web Components 样式隔离(清单 §一.5-2,独立一轮);
2. `animationend`/`transitionend` 事件派发(JS 收口回调);
3. transform 矩阵分解插值(rotate 大角度正确路径);
4. `steps()`/`frames()` 缓动、`animation-composition`;
5. rAF 帧对齐(与渲染帧节拍统一,需评估站点启动链风险)。
