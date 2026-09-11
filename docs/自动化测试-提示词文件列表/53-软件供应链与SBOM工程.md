# 自动化测试提示词 — 软件供应链与 SBOM 工程（BB01–BB10）

> 使用说明:聚焦第三方依赖治理:SBOM 生成 / CVE 监控 / 制品签名 / License 合规 / SLSA provenance / 供应链投毒防御;每条要求 laew 写可跑脚本解析 lock 文件、生成 SBOM、签名/验签、版本区间比较,并用真实统计/CVE 匹配准确率断言。供应链攻击事件落盘为案例库,防御脚本断言必填字段。

## 维度说明
考察 laew 把供应链工程落成可验证脚本的能力:数据→解析→统计/匹配→落盘产物→断言。lock 文件解析必须可独立运行(`python3 -m` 无外网),CVE 用本地小库做版本区间比较,签名用 Python 标准库 `hmac`。

---

### BB01 软件供应链攻击全景与典型事件
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: medium
- **考察维度**: 攻击事件案例库 / STRIDE 落盘
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/sc/events.yaml` 用 YAML 写 6 起真实供应链事件:`solarwinds/codecov/event-stream/ua-parser-js/xcode-go-priv/3cx`,每条 `name|year|stage|source|build|distribute|impact`。
  2. `Bash` 跑 `grep -c '^- name:' events.yaml` 断言恰好 6 起,`awk -F'|' '/stage/{print $3}' events.yaml | sort -u | wc -l` 断言攻击环节 ≥3 类。
  3. `Read` events 后用 STRIDE 模型对 laew 自身依赖(tokio/serde/reqwest/crossterm/rusqlite)写 `stride.md`,每依赖列 Spoofing/Tampering/Repudiation/Info/DoS/Elevation 6 列;`Bash` 跑 `grep -c '^|' stride.md` ≥ 8(表头+6 依赖)。
  4. `Bash` 用 `awk` 数每依赖「高风险 Y」计数,选出 top 3 写入 `top3.md`(`grep -B0 -A0 '^| Y ' stride.md | head` 验证),格式不对则修正。

### BB02 SBOM 标准与生成工具
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: medium
- **考察维度**: lock 文件解析 / SBOM CSV 产出
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/sbom/Cargo.lock.sample` 用 heredoc 写一个仿 Cargo.lock 片段(15 个 `[[package]]` 条目,字段 `name/version/source`)。
  2. `Write` `parse_lock.py` 解析该文件,输出 `sbom.csv`(列 `name,version,source`),`Bash` 跑 `python3 parse_lock.py` 生成 CSV,`wc -l sbom.csv` 断言 = 16(表头+15 包)。
  3. `Read` 后加 CycloneDX JSON 格式输出 `sbom.json`(每组件含 `name/version/purl`),`Bash` 跑 `python3 -c "import json;d=json.load(open('sbom.json'));assert d['bomFormat']=='CycloneDX';assert len(d['components'])==15"`。
  4. 再写 `stats.py` 统计:每个 source 的包数、版本多样性(unique/total),`Bash` 跑后 `head` 输出写入 `stats.md`,`awk` 数每行字段数 ≥3,缺失则补齐。

### BB03 依赖漏洞监控与自动修复
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: medium
- **考察维度**: 本地 CVE 库 / 版本区间匹配
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/cve/local_cve.json` 写 5 条漏洞记录:每条 `id|package|severity|affected_ranges(列表如 [">=1.0, <1.2.3"])|fixed_in`。
  2. `Read` 后写 `match.py`:给定一个「项目 lock 解析出的 package+version」列表,对每条 CVE 做版本区间比较(`>= X 且 < Y`),命中则输出告警。
  3. `Write` `project_lock.txt` 含 8 条 (package, version),其中 3 条故意命中 CVE,`Bash` 跑 `python3 match.py` 后 `grep -c WARN out.txt` 断言恰好 3 条告警。
  4. 漏报或多报时调整 `>=`/`<` 半开区间比较(`packaging.version.Version` 或手写 semver),重跑直到告警数 = 3,再写 `auto_fix.md` 决策表(自动 bump/需迁移/人工评估 三档各 2 例子)。

### BB04 SBOM 最小可行实施
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: simple
- **考察维度**: 落地 SOP 文档 / 阶段字段核对
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/sbom-sop/sop.md` 写一份 3 阶段 SBOM 落地 SOP:阶段一生成、阶段二消费、阶段三治理,每阶段 4 项必做工作(含 owner/SLA/工具)。
  2. `Bash` 跑 `grep -cE '^## 阶段[一二三]' sop.md` 断言恰好 3 个阶段标题,`grep -cE '^- \[ \]' sop.md` 断言至少 12 项 checkbox(每阶段 4 项)。
  3. `Write` `check_sop.py` 校验 sop.md:每阶段必须含 owner/SLA/工具三个二级关键词(`grep -c owner` 等),`Bash` 跑 `python3 check_sop.py` 退出码 0。
  4. 故意在 sop.md 删掉阶段三的 owner,重跑 check_sop.py 必须非 0 且 stderr 指出 `阶段三 缺 owner`,补齐后再跑恢复 0,产物存为 `sop_checked.md`。

### BB05 制品签名与 Provenance(来源证明)
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: hard
- **考察维度**: hmac 签名验签 / 篡改检测
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/sig/release.tar` 用 `tar -cf` 打包一个空文件 + `metadata.json`(含 name/version/builder/timestamp),`Bash` 跑 `tar -tf release.tar` 验证含 2 个成员。
  2. `Write` `sign.py` 用 `hmac.new(secret, digestmod=hashlib.sha256)` 计算 tar 字节的签名,写出 `release.sig`(hex 格式)。
  3. `Bash` 跑 `python3 sign.py` 生成签名,`Read` 后写 `verify.py` 加载 secret 重算并比对,`python3 verify.py` 必须 print `OK` 且退出码 0。
  4. 篡改测试:`echo extra >> release.tar` 后再跑 verify 必须 print `TAMPERED` 且退出码非 0,修复:重新签一次并把整个流程写入 `provenance.md`(含 SLSA L1-L3 三段,每段 ≥5 行)。

### BB06 License 合规与法务审计
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: medium
- **考察维度**: License 分级 / 合规扫描脚本
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/lcs/licenses.json` 写 8 个依赖的 `name|license|version`,其中 2 个 GPL、1 个 AGPL、5 个 MIT/Apache。
  2. `Read` 后写 `audit.py`:三类规则(`allowed: MIT/BSD/Apache/MPL`、`forbidden: AGPL`、`review: GPL`),输出每依赖 risk=`allow/forbidden/review`。
  3. `Bash` 跑 `python3 audit.py` 后 `grep -c 'forbidden' out.txt` = 1(AGPL),`grep -c 'review' out.txt` = 2(GPL),其余 = allow。
  4. `Write` `audit_report.md` 含每依赖的风险级别与处置建议(替换/隔离/许可谈判),`Bash` `grep -cE '^## ' audit_report.md` ≥ 4(概览+三类+总结),缺失则补齐。

### BB07 私有依赖仓库与镜像治理
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: medium
- **考察维度**: 白名单脚本 / typosquatting 检测
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/mirror/allowlist.txt` 写允许的镜像源 3 行(`crates.io`、`internal-mirror.local`、`gov-allowed.cn`),`Bash` 跑 `wc -l allowlist.txt` 断言 = 3。
  2. `Write` `urls.txt` 含 5 个待审计 URL(3 个在白名单内、2 个外站),`Read` 后写 `validate.py` 判断每个 URL 是否在白名单(`subdomain.allow` + `exact.match`)。
  3. `Bash` 跑 `python3 validate.py` 后 `grep -c 'ALLOW' out.txt` = 3,`grep -c 'DENY' out.txt` = 2。
  4. 加 typosquatting 检测:对照已知包名 `tokio/serde/reqwest`,把 `requwest/tokki/serder` 加入 `suspect.txt`,`Write` `typo_check.py` 用编辑距离 ≤2 命中,`Bash` 跑 `python3 typo_check.py` 断言命中数 = 3。

### BB08 依赖版本固定与可复现构建
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: medium
- **考察维度**: lock 哈希校验 / 可复现性断言
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/lock/Cargo.lock` 写 5 条 `[[package]]`(name/version/checksum),`Bash` 跑 `grep -c 'name =' Cargo.lock` 断言 = 5。
  2. `Write` `verify_lock.py` 校验:每条 name 唯一、`checksum` 字段非空、版本符合 semver,`Bash` 跑 `python3 verify_lock.py` 退出码 0。
  3. 故意造一份 `Cargo.lock.bad`:删一条 checksum 字段,重跑必须非 0 且 stderr 输出缺失字段的包名,验证检测有效。
  4. `Read` 后补一份 `repro.md` 列「可复现构建三支柱」:源码确定(Cargo.lock + git sha)、环境确定(Dockerfile 固定 base)、时序无关(SOURCE_DATE_EPOCH),`Bash` `grep -cE '^## 三支柱|^### ' repro.md` ≥ 3。

### BB09 依赖最小化与供应链瘦身
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: simple
- **考察维度**: 价值/风险评分 / 瘦身前后对比
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/slim/deps.tsv` 写 6 个依赖的 `name|monthly_downloads|last_release|known_cves|license`。
  2. `Read` 后写 `score.py`:计算 value/risk 比(value = downloads × recency,risk = cves × license_weight),输出排序后的 `ranked.tsv`。
  3. `Bash` 跑 `python3 score.py` 后 `head -n 1 ranked.tsv` 必须是得分最高的依赖(预期 name=`serde` 之类高频活跃);`tail -n 1 ranked.tsv` 必须是最低的。
  4. `Write` `slim_plan.md`:列出 2 个「可移除/可替换」的瘦身建议(基于 ranked 末位),每项含替换方案与影响评估,`Bash` `grep -c '^## ' slim_plan.md` ≥ 3。

### BB10 供应链攻击应急响应与事件复盘
- **测试状态**: 🔄 待重测（2026-09-11 脚本重写）
- **预期档位**: hard
- **考察维度**: Runbook 落盘 / 应急剧本结构
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ir/runbook.md` 写一份供应链事件 Runbook:5 阶段 IR(检测-遏制-根除-恢复-复盘)每阶段 4 步,共 20 步。
  2. `Bash` 跑 `grep -cE '^### ' runbook.md` 断言 ≥ 25(阶段标题+每阶段 4 子步),`grep -cE '^## ' runbook.md` 断言 = 5(五阶段)。
  3. `Read` 后写 `check_runbook.py` 校验:每阶段必须含「关键动作」「决策点」「时间预算」三个关键词,`Bash` 跑必须 0。
  4. 故意删掉恢复阶段的「时间预算」段,重跑 check_runbook.py 必须非 0 指出恢复阶段缺失,补齐后再跑恢复 0,产物 `runbook_validated.md` 末尾追加「log4j2 复盘时间线」6 段。