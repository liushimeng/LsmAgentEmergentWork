# 53 软件供应链与 SBOM 工程

> 编号段 BB01–BB10 · 聚焦第三方依赖治理：SBOM 生成 / CVE 监控 / 制品签名 / License 合规 / SLSA provenance / 供应链投毒防御
> 与现有 29 维「云原生与 DevOps 工程实战」互补：29 偏 CI/CD 与编排；53 偏制品级供应链与安全合规
> 与现有 47 维「安全工程与渗透测试」互补：47 偏运行时攻防；53 偏构建时与发布前防御

---

### BB01 软件供应链攻击全景与典型事件
- **预期档位**: medium
- **考察维度**: 攻击史 + 攻击面建模
- **对话脚本**:
  1. 软件供应链的三层定义：源代码层（开源依赖/内部代码）、构建层（CI/CD/构建工具链）、发布层（包管理器/容器镜像），每层给一个 2020-2026 年的真实事件。
  2. SolarWinds、CodeCov、event-stream、ua-parser-js、xcode-go-priv、3CX Supply Chain 六大事件的攻击向量对比：哪个是源码污染、哪个是构建注入、哪个是分发劫持？
  3. 供应链攻击的「经济学」：攻击单家公司 vs 攻击通用依赖库的 ROI 差异，为什么恶意维护者主动投递后门的案例在 2024 年激增？
  4. 用 STRIDE 模型给 laew 的依赖树（tokio/serde/reqwest/crossterm/rusqlite）做一次供应链威胁建模，列出 5 个最高风险的攻击向量。

### BB02 SBOM 标准与生成工具
- **预期档位**: medium
- **考察维度**: SPDX / CycloneDX / 工具链对比
- **对话脚本**:
  1. 两大 SBOM 标准对比：SPDX（Linux Foundation）vs CycloneDX（OWASP）的格式差异、生态覆盖、扩展字段（License / Vulnerability / Dependency Graph）。
  2. 主流 SBOM 生成工具实测：Syft / Trivy / cdxgen / SPDX Tools / Microsoft SBOM Tool 在 Rust/Node/Java/Go 项目上的生成准确率、字段完整度、CI 集成成本。
  3. SBOM 的下游消费场景：漏洞扫描（Grype/Trivy）、License 合规扫描（FOSSA/Snyk）、依赖关系可视化（Eclipse SW360）、采购方交付（政府采购/医疗合规）。
  4. 给 laew 编写一份 SBOM 生成流水线：每次 release tag 触发 GitHub Actions，跑 `cargo cyclonedx` 生成 CycloneDX JSON，归档到 release artifacts 并发布到内部 SBOM 仓库。

### BB03 依赖漏洞监控与自动修复
- **预期档位**: medium
- **考察维度**: CVE 数据库 + 自动 PR
- **对话脚本**:
  1. 主流漏洞数据库对比：NVD（美国国家漏洞库）/ GHSA（GitHub Security Advisory）/ OSV（Google 开源漏洞）/ RustSec（Rust 生态专用）/ China 漏洞库（CNNVD），覆盖度与时延差异。
  2. SCA 工具实测：Snyk / Dependabot / GitLab Dependency Scanning / Renovate / Mend（WhiteSource）的误报率、修复 PR 成功率、对私有 registry 的支持对比。
  3. 自动修复的边界：哪些 CVE 能自动 bump 版本、哪些需要代码迁移（breaking change）、哪些必须人工评估（内核/加密库），给一个分级决策矩阵。
  4. 给 laew 设计一套依赖巡检：每周 cron 跑 `cargo audit` + `cargo outdated` + OSV-Scanner，产出报告自动发到飞书群，CRITICAL 级自动开 Issue 跟踪。

### BB04 软件物料清单的最小可行实施
- **预期档位**: medium
- **考察维度**: SBOM 落地路径 + 团队协同
- **对话脚本**:
  1. SBOM 落地的三阶段：阶段一生成（手动 + 自动）→ 阶段二消费（漏洞/许可扫描）→ 阶段三治理（自动修复 + SLA + 审计），每阶段的最小投入估算。
  2. SBOM 的存哪里：与制品同仓库（oci artifact）/ 独立 SBOM 仓库 / 第三方平台（FOSSA/Dependency-Track）的对比，给一个 50 工程师团队的最优解。
  3. SBOM 与「数字护照」（Digital Product Passport，欧盟 2026 强制）的衔接：消费品、工业品、软件三类的合规要求差异。
  4. 编写一份 SBOM 治理 SOP：从新依赖引入评审（License/CVE/维护活跃度）、季度审计（未使用依赖清理）、年度合规报告输出的完整流程文档。

### BB05 制品签名与 Provenance（来源证明）
- **预期档位**: hard
- **考察维度**: Sigstore Cosign + SLSA L3
- **对话脚本**:
  1. 制品签名的三大流派：PGP（传统/手动/兼容性差）、X.509 + PKI（企业级/复杂）、Sigstore（现代/无密钥/OIDC 绑定）的工程权衡。
  2. Sigstore 工具链三件套：Cosign（镜像/制品签名）、Rekor（透明日志/不可篡改）、Fulcio（短期证书签发）的工作链路，给一个完整示例。
  3. SLSA 框架 L1-L4 等级详解：L1（构建脚本化）、L2（来源可追溯）、L3（来源防篡改）、L4（双 Review + 隔离构建）每级需要的工程改造与成本。
  4. 给 laew 设计签名流水线：cargo build → 生成 SBOM → SLSA provenance（GitHub OIDC）→ cosign sign --rekor，验证脚本镜像的完整性，给出 GitHub Actions YAML。

### BB06 License 合规与法务审计
- **预期档位**: medium
- **考察维度**: OSI 许可证 + 合规扫描
- **对话脚本**:
  1. 主流开源 License 分级：宽松型（MIT/BSD/Apache 2.0）、弱传染型（MPL/EPL）、强传染型（GPL/LGPL/AGPL），商业产品对各类 License 的接受度矩阵。
  2. License 冲突的典型场景：GPL 库静态链接进闭源软件、AGPL 服务暴露在公网、SSPL（MongoDB）/ BUSL（Elastic）的实际法律效力争议。
  3. License 合规扫描工具对比：FOSSA / Snyk License Compliance / ScanCode / FOSSology 的 License 识别准确率、误报处理、私有 License 模板支持。
  4. 给 laew 做一次 License 审计：扫描 Cargo.toml 所有依赖的 License，输出一份合规清单（含允许/禁止/需评审三类），标记每个 License 的风险等级。

### BB07 私有依赖仓库与镜像治理
- **预期档位**: medium
- **考察维度**: 制品库 + 镜像治理
- **对话脚本**:
  1. 主流制品仓库对比：Nexus / Artifactory / Harbor（容器）/ Cloudsmith / Gemfury / Verdaccio（npm）的多语言支持、HA 部署、License 模型。
  2. 私有 Cargo Registry 搭建：`cargo-private-registry`（GitHub 服务）/ Verdaccio 适配 / Axum 自研 / crates.io 企业镜像四种方案的成本与维护负担。
  3. 镜像治理三大难题：上游源安全（恶意包投毒）、镜像同步延迟、断网/限网环境下的本地依赖缓存，给 laew（已用 rsproxy.cn）写一份镜像治理 SOP。
  4. 设计一个企业级「可信源」治理：白名单上游仓库（只允许 crates.io / 私有仓库 / 商务部认证源）、包名黑名单（typosquatting 检测）、自动镜像签名验证。

### BB08 依赖版本固定与可复现构建
- **预期档位**: medium
- **考察维度**: 锁文件 + 确定性构建
- **对话脚本**:
  1. 锁文件机制对比：Cargo.lock / package-lock.json / Pipfile.lock / go.sum / Gemfile.lock 的字段完整性、跨平台一致性、合并冲突频率。
  2. 可复现构建（Reproducible Build）的三大支柱：源码确定（lock + commit hash）→ 构建环境确定（Docker 固定 base image + 工具版本）→ 时序无关（SOURCE_DATE_EPOCH）。
  3. 真实事故：`left-pad`（2016, npm 11 行代码下线）/ `colors.js` 故意破坏 / `node-ipc` 投毒事件，依赖治理缺失对工程团队的实际冲击。
  4. 给 laew 配置可复现构建：`Cargo.lock` 入库 + GitHub Actions `actions/cache` + `cargo --locked` + Docker 多阶段固定 Rust toolchain 版本，给一份完整 CI 配置。

### BB09 依赖最小化与供应链瘦身
- **预期档位**: medium
- **考察维度**: 依赖治理 + 攻击面收敛
- **对话脚本**:
  1. 「依赖即负债」：评估每个第三方依赖的「价值/风险比」——开发效率收益、维护活跃度、安全历史、License 风险四维度打分。
  2. 常见瘦身策略：tree shaking（JS）/ `cargo tree --duplicates` 去重 / `npm prune --production` / 静态链接剥离 debug 符号的实测效果。
  3. 「自研 vs 复用」的决策矩阵：何时该自研一个小工具（隐私/可控/审计）、何时该用成熟库（速度/质量/生态），给 laew 的「三个内置工具」（Bash/Read/Write）的设计意图分析。
  4. 给 laew 做一次「依赖瘦身体检」：用 `cargo tree` 分析依赖图，找出 5 个可以替换或移除的传递依赖，估算每次瘦身后编译时间和二进制大小的变化。

### BB10 供应链攻击应急响应与事件复盘
- **预期档位**: hard
- **考察维度**: IR 流程 + 复盘方法论
- **对话脚本**:
  1. 供应链事件的「检测-遏制-根除-恢复-复盘」五阶段 IR（Incident Response）流程，每个阶段的关键动作、决策点、时间预算。
  2. 真实复盘：log4j2（2021, CVE-2021-44228）的事件时间线——从漏洞披露到全网补丁用了多久、企业级响应剧本的关键步骤。
  3. 应急修复策略对比：版本升级 vs 虚拟补丁（WAF/规则）vs 配置缓解 vs 下线服务，给出每个策略的适用场景与权衡。
  4. 编写一份 laew 供应链事件 Runbook：检测到核心依赖出现 RCE 级别 CVE 时的 24 小时响应剧本——含决策树、沟通模板、回滚方案、复盘模板。
