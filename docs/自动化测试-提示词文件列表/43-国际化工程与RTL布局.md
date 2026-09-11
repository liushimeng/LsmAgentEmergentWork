# 自动化测试提示词 — 国际化工程与 RTL 布局（AR01–AR10）

> 使用说明见同目录 `README.md`。每条提示词在同一 Session 内按轮次顺序喂入。

## 维度说明

本维度考察 Agent 在 **i18n/l10n 工程化** 上的动手能力：能写 ICU MessageFormat 子集解析器、能用 Python 字符串 bidi 逻辑处理 RTL 文本、能用 zoneinfo 处理 DST 时区、能用 unicode 规范化做多语言匹配。
所有产物落到 `tmpPlan/agent-test/` 沙盒，不依赖外网/翻译 API/浏览器。
与 README 其它维度互补：E 考察通用编码，AR 聚焦"语言学 + 字符层"——复数规则、双向算法、字符集规范化、时区数据。

---

### AR01 i18n 与 l10n：写一个 gettext 风格 mini 提取器
- **预期档位**: simple
- **考察维度**: 字符串提取 + key 命名规范
- **工具链**: Write → Bash → Bash → Read
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ar01/` 写一个含中文/英文/阿拉伯文硬编码字符串的 sample.py（如 `print("欢迎回来")`、`print("Welcome back")`、`print("مرحبا")`），用 Write。
  2. 写 `extract.py`：扫描 sample.py，把所有 `"..."` 字符串字面量提取成 key=value 形式（key 用行号+前 4 个字符 MD5 前 6 位），输出到 `messages.pot`。
  3. 跑 `python3 extract.py sample.py`，用 `wc -l messages.pot` 断言行数等于源文件字符串数（≥3）。
  4. 在 `key_design.md` 写 key 设计哲学对比：英文句子 vs 数字 ID vs 语义命名（`home.title.welcome`），结合本文件提取结果各举 1 例。

### AR02 ICU MessageFormat 子集：复数/选择/参数
- **预期档位**: medium
- **考察维度**: ICU 语法解析 + 多语言复数
- **工具链**: Write → Bash → Bash → Read
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ar02/` 写 `mini_icu.py`：实现 ICU MessageFormat 的最小子集解析器，支持 `{name}` 占位符替换 + `{count, plural, one {1 件} other {# 件}}` 复数 + `{gender, select, male {他} female {她} other {TA}}` 选择，函数签名 `format(template: str, args: dict, locale: str) -> str`。
  2. 写 `test_icu.py` 用 `unittest`：至少 6 个 case（en 复数 0/1/2、ar 复数 zero/one/two/many/other、gender 三态、空 args fallback）。
  3. 跑 `python3 -m unittest test_icu.py -v`，断言全过（`OK`），失败用例打印出 locale + 期望 + 实际。
  4. 在 `plurals.md` 写一段对比：英文 `one/other` 2 形 vs 阿拉伯文 `zero/one/two/few/many/other` 6 形规则差异，给出 `n=0/1/2/3/11/100` 六种 n 在 ar locale 下的实际选中分支。

### AR03 RTL 双向文本：纯逻辑布局模拟
- **预期档位**: medium
- **考察维度**: Unicode BiDi 逻辑 + 嵌入控制字符
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ar03/` 写 `bidi.py`：用 Python 字符分类（`unicodedata.bidirectional()`）把字符串拆成 LTR/RTL 段，按 Unicode BiDi 简化规则反向 RTL 段，输出"逻辑序"和"视觉序"两个版本。
  2. 测试用例：输入 `"ABC 123 مرحبا def"`，跑 `python3 bidi.py` 输出两行到 `bidi.txt`，断言视觉序中"مرحبا"位于数字之前（应在 123 之前）。
  3. 用嵌入控制字符：把 `"file_اسم.txt"` 包成 `"‭file_اسم.txt‬"`，再跑一次 bidi.py，断言视觉序为 `"file_اسم.txt"`（LTR 嵌入保护）。
  4. 在 `layout.md` 写 LTR vs RTL 表单镜像对比：label/input/button 三者在两套布局下各自的"逻辑坐标 → 视觉坐标"映射，列 3 行。

### AR04 翻译键设计：写命名 + 校验脚本
- **预期档位**: medium
- **考察维度**: key 命名哲学 + 翻译键扫描
- **工具链**: Write → Bash → Bash → Read
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ar04/` 写 `app.py` 含 3 类字符串：扁平 key（`"home.welcome"`）、嵌套 key（`"order.detail.total"`）、ID 风格（`"msg_0042"`），每类至少 2 处。
  2. 写 `lint.py` 实现翻译键 lint：扫描 app.py，检测 `home.welcome` 与 `home.welcom` 相似度（编辑距离 ≤2 → 警告）、检测重复 key、检测 key 含空格或大写，把违规写到 `lint_report.txt`。
  3. 跑 `python3 lint.py app.py`，用 `grep -c 'WARNING\|ERROR' lint_report.txt` 断言至少 1 条警告。
  4. 在 `key_philosophy.md` 写决策矩阵：英文句子/数字 ID/语义命名三种 key 风格在"易读/重构成本/翻译者友好/可搜索"4 维度的对比，每维给 1-5 分。

### AR05 字体回退栈：按字符范围动态选择
- **预期档位**: medium
- **考察维度**: 字体回退算法 + Unicode block
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ar05/` 写 `fallback.py`：定义回退栈 `["NotoSansCJK", "NotoNaskhArabic", "NotoSansDevanagari", "NotoSans", "sans-serif"]`，函数 `pick_font(char) -> str` 按 `unicodedata.name(char)` 决定字体。
  2. 测 5 种字符：'你'(CJK) → CJK、'م'(Arabic) → NaskhArabic、'अ'(Devanagari) → Devanagari、'A'(Latin) → NotoSans、'😀'(Emoji) → sans-serif，写 `test_pick.py` 用 `unittest` 断言。
  3. 跑测试全过，输出 "OK"；故意把 CJK 期望改错后重跑，断言输出 "FAILED" 且有清晰 diff。
  4. 在 `fallback_notes.md` 写"为什么 NotoSansCJK 不覆盖 Arabic"的原理（block 范围差异）+ emoji 字体优先级建议（Apple/Google/Microsoft 的家族名）。

### AR06 时区与 DST：zoneinfo 端到端验证
- **预期档位**: medium
- **考察维度**: IANA tzdata + DST 切换
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ar06/` 写 `dst_check.py`：用 `zoneinfo.ZoneInfo` 遍历 `["America/New_York", "Europe/London", "Asia/Shanghai", "Pacific/Auckland"]` 在 2026-03-08、2026-11-01 两个跨 DST 日期的 `utcoffset()` 与 `dst()`。
  2. 跑 `python3 dst_check.py` 输出到 `dst.txt`，每行 `tz date offset dst_flag`，断言 New_York 在 03-08 offset=-04:00 11-01 offset=-05:00（夏令时切换），Shanghai 全年 offset=+08:00 dst_flag=False。
  3. 写 `meeting.py` 实现"每周一上午 10:00 在 Asia/Shanghai"调度：给定一个 UTC 基准时间，输出接下来 4 次触发对应的 America/Los_Angeles 本地时间，写到 `meetings.txt`。
  4. 在 `dst_notes.md` 写"为什么只用 UTC 偏移不够"：列出 3 个真实时区变动事件（如 Samoa 2011 跳过 12-30、Morocco 2018 斋月切换、Russia 2014 取消冬令时），每条 1 行。

### AR07 文本规范化与多语言搜索
- **预期档位**: simple
- **考察维度**: Unicode NFC/NFD + 大小写折叠
- **工具链**: Write → Bash → Bash → Read
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ar07/` 写 `normalize.py`：实现 `normalize(query)` 函数，用 `unicodedata.normalize("NFC", s)` + `casefold()` + 去除重音（`́` 组合用 `s.translate(str.maketrans(...))` 或 `re.sub("́", "", ...)`）。
  2. 测试三种场景：(a) "Björn" 与 "Bjorn" 匹配、(b) "Café" 与 "Cafe" 匹配、(c) 中文"北京"与"北 京"（中间夹空格）匹配；写 `test_norm.py` 用 unittest 断言 3/3 通过。
  3. 跑 `python3 -m unittest test_norm.py -v`，输出全 OK；故意把 casefold 改成 lower() 后断言中文 case 失败（演示 lower 对非 ASCII 不安全）。
  4. 在 `norm_notes.md` 写"NFC vs NFD"区别：café 两种编码的字节序列对比，hexdump 贴进文档。

### AR08 RTL 数字与邮箱：嵌入方向控制
- **预期档位**: medium
- **考察维度**: 数字 LTR 嵌入 + 控制字符
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ar08/` 写 `wrap_ltr.py`：函数 `wrap(s)` 把数字串、邮箱、文件路径自动包成 `‭...‬`（LRE/PDF）。
  2. 测试 `"اتصل على 123-4567"` → `"اتصل على ‭123-4567‬"`；`"راسل user@example.com"` → 邮箱整体被包。
  3. 跑 `python3 wrap_ltr.py` 输出到 `wrapped.txt`，断言至少 2 个 `‭` 与 2 个 `‬` 出现（用 `grep -c` 数）。
  4. 在 `rtl_pitfalls.md` 写 3 个常见 RTL 布局陷阱：图标方向（箭头、播放钮需镜像）、阴影方向（box-shadow 偏移翻转）、动画方向（slide-in RTL 下需 -100%），每条 1 行 + 修复方案。

### AR09 复数规则测试：多 locale 并行
- **预期档位**: medium
- **考察维度**: CLDR 复数规则简化实现
- **工具链**: Write → Bash → Bash → Read
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ar09/` 写 `plurals.py`：实现 `plural_form(n, locale) -> str`，内置 en(1 形: one/other)、zh(1 形: other)、ar(6 形: zero/one/two/few/many/other)、ru(4 形: one/few/many/other)、pl(3 形: one/few/other) 五个 locale 的规则。
  2. 写 `test_plurals.py` 用 unittest：每个 locale 跑 `n=0,1,2,3,5,11,21,100` 8 个值，断言每个 n 返回的分支符合 CLDR（ar 的 11→many、ru 的 11→many、pl 的 12→many）。
  3. 跑测试全过，输出 OK；故意把 ar 的 11 错选为 few（应 many），重跑断言测试失败并指出具体 n 与 locale。
  4. 在 `cldr_notes.md` 写 ar 6 形触发条件：zero(n=0)、one(n=1)、two(n=2)、few(n%100∈3..10)、many(n%100∈11..99)、other(其他)，每行 1 个公式。

### AR10 伪本地化与翻译覆盖率
- **预期档位**: medium
- **考察维度**: 伪本地化检测 + 覆盖率统计
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ar10/` 写 `pseudo.py`：把英文句子 `s` 转成伪本地化 `transform(s) = "[" + s + "]" + "áĉĉ".repeat(len(s)//2)`（无重音/无重音化变长便于发现 UI 截断）。
  2. 测试 `"Sign in"` → `"[Sign in]áĉĉ"`，`"Welcome to laew"` → 含 6+ 字符变长；输出到 `pseudo.txt`。
  3. 写 `coverage.py`：扫描 `app.py`（含 5 个 `t("...")` 调用），对照 `translations.json`（含 3 条已翻译），输出 `coverage=60%` 到 `coverage.txt`，断言数字 60。
  4. 在 `pseudo_notes.md` 写伪本地化三大作用：UI 截断检测、未翻译字符串发现、字符串拼接问题暴露，各 1 行 + 例子。
