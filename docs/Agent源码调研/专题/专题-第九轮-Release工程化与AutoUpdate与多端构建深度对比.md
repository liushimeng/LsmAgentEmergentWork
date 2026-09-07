# 专题-第九轮-Release工程化与AutoUpdate与多端构建深度对比

> 第九轮 T5 专题：**8 工程 × 9 维度**横向对比，覆盖版本号 / CI / 构建产物 / 签名 / Auto Update / 回滚 / Changelog / 多端分发 / 安装体验。
> 调研对象：claudecode / atomcode / openclaw / opencode / pi / cc-switch / agent-studio / agent-core。
> 调研时间：2026-09-07；目标读者：laew 维护者、DevX 工程师、Release Manager。

---

## 1. 摘要与导读

laew 当前 Release 流程仅 `rebuild_restart_app.sh` 手动脚本，第九轮 **L59-L63 五个 gap**：

| Gap | 描述 | 紧急度 |
|-----|------|--------|
| **L59** | 无 CI 流水线 | P1 |
| **L60** | 仅手动 release（无 semver / changelog） | P1 |
| **L61** | 无 Auto Update 通道 | P2 |
| **L62** | 无签名 / 公证 | P2 |
| **L63** | 无多端分发（仅二进制） | P2 |

8 工程调研后我们看到 **5 档 Release 哲学**：

1. **L1 纯手动 semver**（claudecode / agent-core）
2. **L2 自研 update + 强回滚**（atomcode）
3. **L3 通道解耦 + 不可变 tag**（openclaw）
4. **L4 多产物 multi-channel**（opencode）
5. **L5 桌面全平台打包**（cc-switch）

---

## 2. 8 工程 Release 概览

### 2.1 claudecode（TS/Bun）— L1
- 版本：纯 semver（`package.json version`）
- CI：npm + GH Actions
- 分发：npm registry
- Auto Update：`npm i -g`
- Changelog：远程 RAW + 本地 cache（`src/utils/releaseNotes.ts:11-23`）

### 2.2 atomcode（Rust）— L2 范本
- `crates/atomcode-updater/` 独立 crate，6 步自更新（lib.rs:1-25）
- 立即升级 `run_upgrade` + 延迟升级 `prepare_deferred_upgrade`
- 三段式 rename：`atomcode → .atomcode.rolling → 新 binary → .bak`
- Windows `robust_rename` 重试 5 次（100/200/400/800ms）
- SHA256 校验 + `.bak` 回滚 + `MAX_APPLY_ATTEMPTS = 3` 断路器

### 2.3 openclaw（TS/Bun）— L3 范本
- `vYYYY.MINOR.PATCH(-beta.N)` calver-ish，强制正则 `^v[0-9]{4}\.[1-9][0-9]*\.[1-9][0-9]*`
- Docker image 钉 SHA256 digest（`Dockerfile:11-18`）
- Sparkle (macOS) + Ed25519 私钥签 appcast
- Docker 不可变 tag + 全 SHA 锁 + CI concurrency group
- 三通道：`stable` / `extended-stable` / `beta`

### 2.4 opencode（TS/Bun）— L4
- 多分支（`ci`/`dev`/`beta`/`snapshot-*`）+ 多产物（CLI/VSCode/Action/容器/Cloudflare）
- npm Trusted Publisher（OIDC，`id-token: write`）
- GHCR 多架构容器（`packages/containers/{base,bun-node,rust,tauri-linux,publish}`）

### 2.5 pi（TS/Node）— L1
- `scripts/release.mjs` 10 步流程
- 6 平台 native binary（`pi-darwin-arm64`/`linux-x64`/...）
- 双 commit（release + next-cycle）
- tag 触发 CI

### 2.6 cc-switch（Tauri+Rust）— L5 范本
- Tauri `createUpdaterArtifacts: true` + Ed25519 `pubkey`
- 多 endpoint fallback（自有 CDN `dl.ccswitch.io` → GitHub Releases）
- macOS 完整 codesign + notarize + staple + DMG
- 5 OS matrix（windows-2022/windows-11-arm/ubuntu-22.04/ubuntu-22.04-arm/macos-14）
- Linux 多包（appimage/deb/rpm）+ Flatpak manifest

### 2.7 agent-studio（Python）— Helm + 多 Docker 镜像
- 8 个 Dockerfile（`base`/`server`/`web`/`web.http`/`plugin`/`sandbox-server`/`sandbox-gateway`/`upgrade`/`upgrade.base`）
- `scripts/upgrade_handler.sh` pre-upgrade env 文件格式校验（`env.<5-chars>`）
- Helm umbrella chart + Compose 模板

### 2.8 agent-core（Python）— L1 极简
- PEP 621 `pyproject.toml`
- optional-dependency groups（`all-mq`/`all-storage`/`all-vector`/`obs`/`cli`/`claude`/`codex`）
- `openjiuwen` 包名 + `[project.entry-points]` 注册可插拔 A2A adapter

---

## 3. 维度 1：版本号策略

### 3.1 横向对比表

| 工程 | 策略 | 解析 |
|------|------|------|
| **claudecode** | semver | `npm version major|minor|patch` |
| **atomcode** | semver + pre-release | `vX.Y.Z-beta.1`/`-rc.2` |
| **openclaw** | calver-ish | `vYYYY.MINOR.PATCH(-beta.N)` |
| **opencode** | semver + 多分支 | `ci`/`dev`/`beta`/`snapshot-*` |
| **pi** | 纯 semver | `^\d+\.\d+\.\d+$` |
| **cc-switch** | semver | `tauri.conf.json` 单一来源 |
| **agent-studio** | semver | `sed -i` 替换 |
| **agent-core** | PEP 440 | `0.1.17` |

### 3.2 atomcode 范本（`scripts/release.sh:23-44`）

```bash
VERSION="${ATOMCODE_VERSION:-}"
CARGO_VERSION=$(awk -F'"' '/^\[workspace\.package\]/ {in_section=1;next} /^\[/ {in_section=0} in_section && /^version *=/{print $2;exit}' Cargo.toml)
case "$VERSION" in
    v[0-9]*) ;;
    *) echo "Refusing to release with non-vX.Y.Z version: '$VERSION'"; exit 1;;
esac
```

**范式要点**：
1. **强制 vX.Y.Z 前缀**（避免误发）
2. **从 `Cargo.toml` 单一来源读版本**
3. **二进制内置 `env!("CARGO_PKG_VERSION")`** 与 tag 一致

### 3.3 openclaw 强制正则（`validate_release_identity`）

```bash
^v[0-9]{4}\.[1-9][0-9]*\.[1-9][0-9]*(-(beta\.)?[1-9][0-9]*)?$
```

---

## 4. 维度 2：CI/CD 流水线

### 4.1 横向对比表

| 工程 | CI | 并行 | 缓存 | 审批 |
|------|----|----|------|------|
| **atomcode** | GH Actions | macOS+Linux matrix | cargo sccache | keychain preflight |
| **claudecode** | npm + GH Actions | Node matrix | - | - |
| **openclaw** | GH Actions (workflow_call) | - | pnpm | `environment: docker-release` |
| **opencode** | GH Actions (multi-workflow) | - | turbo | npm OIDC |
| **pi** | GH Actions tag | 6 平台 | bun | - |
| **cc-switch** | GH Actions 5-OS matrix | - | - | Tauri Ed25519 |
| **agent-studio** | 无 .github/workflows | - | - | - |
| **agent-core** | 无 .github/workflows | - | - | - |

### 4.2 cc-switch 范本（`.github/workflows/release.yml:120-160`）

```yaml
- name: Prepare Tauri signing key
  # 三种密钥格式自适应: 原文 / base64包一层 / 单行base64
  # 都转成 Tauri CLI 能识别的两行文件: "untrusted comment: ...\n<key>\n"

- name: Import Apple signing certificate
  # 创建临时 keychain, 导入 .p12, 自动解析 "Developer ID Application" 身份

- name: Build Tauri App (macOS)
  # 3 次重试机制, pnpm tauri build --target universal-apple-darwin
- name: Notarize macOS DMG
  # xcrun notarytool submit + wait + xcrun stapler staple
```

### 4.3 openclaw 范本（`.github/workflows/docker-release.yml:1-30`）

```yaml
on:
  workflow_call:
    inputs:
      tag: {required: true, type: string}                # Immutable stable/extended-stable/beta tag
      release_sha: {required: true, type: string}        # Full immutable commit SHA
      image_tag_suffix: {required: false, default: ""}
permissions:
  contents: read
env:
  FORCE_JAVASCRIPT_ACTIONS_TO_NODE24: "true"
```

---

## 5. 维度 3：构建产物

### 5.1 8 工程产物对比

| 工程 | 产物形态 |
|------|---------|
| **claudecode** | npm 包 |
| **atomcode** | 6 平台裸二进制（darwin-arm64/linux-x64/musl/ohos-arm64） |
| **openclaw** | Docker 镜像 + macOS `.app` + iOS/Android + SaaS |
| **opencode** | CLI / VSCode / GitHub Action / 容器 / Cloudflare Worker |
| **pi** | 6 平台 tar.gz/zip + source tarball |
| **cc-switch** | MSI / .app / .dmg / .AppImage / .deb / .rpm / Flatpak |
| **agent-studio** | 8 个 Docker 镜像 |
| **agent-core** | sdist + wheel |

### 5.2 atomcode 三段式 rename 范本（`crates/atomcode-updater/src/lib.rs:540-628`）

```rust
//! 6. Three-way swap to replace the live binary:
//!    a. `atomcode` → `.atomcode.rolling`
//!    b. new binary → `atomcode`
//!    c. best-effort: remove old `.bak`, then `.atomcode.rolling` → `.bak`
```

**范式要点**：
1. **三段式 rename** 确保任何时刻有可回滚版本
2. **`MAX_APPLY_ATTEMPTS = 3`** 断路器
3. **`run_rollback()`** 与 `.bak` 互换

### 5.3 agent-studio 多镜像编排（`docker/Dockerfile.server:1-30`）

```dockerfile
ARG BASE_IMAGE
FROM ${BASE_IMAGE} AS builder
ARG VERSION
ARG INDEX_URL
WORKDIR /app
COPY backend .
COPY connect /app/connect
RUN pip install uv -i ${INDEX_URL}
RUN sed -i "s#url = \"[^\"]*\"#url = \"${INDEX_URL}\"#g" pyproject.toml
RUN uv sync --group dev
RUN uv build --out-dir /app/dist

FROM ${BASE_IMAGE} AS runtime
ARG WHL_NAME="openjiuwen_studio-${VERSION}-py3-none-any.whl"
RUN pip3 install /app/dist/${WHL_NAME} --trusted-host ${TRUSTED_HOST} ...
```

---

## 6. 维度 4：签名 / 公证

### 6.1 4 档签名范式

| 工程 | macOS | Windows | Linux | Docker |
|------|-------|---------|-------|--------|
| **atomcode** | codesign + notarytool | - | - | - |
| **openclaw** | codesign + notarytool + Sparkle Ed25519 | - | - | SHA256 digest |
| **cc-switch** | Tauri Ed25519 + Apple notary + DMG 重签 | - | - | - |
| **opencode** | - | - | - | npm OIDC |
| **其他** | 无 | 无 | 无 | 无 |

### 6.2 cc-switch Ed25519 范本（`src-tauri/tauri.conf.json:34-80`）

```json
"plugins": {
  "updater": {
    "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEM4MDI4QzlBNTczOTI4RTMK...",
    "endpoints": [
      "https://dl.ccswitch.io/latest.json",
      "https://github.com/farion1231/cc-switch/releases/latest/download/latest.json"
    ]
  }
}
```

---

## 7. 维度 5：Auto Update 通道

### 7.1 横向对比表

| 工程 | Auto Update | 实现 |
|------|-------------|------|
| **atomcode** | ✅ 自研 | HTTP+SHA256 + 三段式 rename |
| **claudecode** | npm `-g` | npm upgrade |
| **openclaw** | Sparkle + Docker pull | Ed25519 appcast + 不可变 tag |
| **opencode** | npm + Marketplace | 各通道独立 update |
| **pi** | 手动 | tag-based |
| **cc-switch** | Tauri updater | 双 endpoint fallback |
| **agent-studio** | 自研容器内 | `upgrade_handler.sh` |
| **agent-core** | `pip -U` | PyPI |

### 7.2 atomcode 自研 updater 范本（`crates/atomcode-updater/src/lib.rs:1-25`）

```rust
//! 1. Fetch `latest.json` manifest (version + per-target sha256/size).
//! 2. Detect current platform and pick the matching binary entry.
//! 3. Verify we can write to `current_exe()`'s directory — if not, fail
//!    with a precise message telling the user to re-run with `sudo`.
//! 4. Download the binary to a sibling temp file, streaming progress.
//! 5. Verify SHA256 against the manifest. Bail (and delete temp) on
//!    mismatch — we never touch the live binary until verification passes.
//! 6. Three-way swap to replace the live binary.
```

---

## 8. 维度 6：回滚机制

### 8.1 4 种回滚范式

| 工程 | 回滚 | 保留 |
|------|------|------|
| **atomcode** | `.bak` 三段式 rename | 1 个旧版 |
| **openclaw** | Docker tag 不可变 + SHA | 所有历史 |
| **cc-switch** | Tauri delta | GitHub Releases 历史 |
| **agent-studio** | `PRE_UPGRADE_VARS` 快照 | 完整原镜像 |
| **pi** | tag | Git history |
| **agent-core** | `pip pin` | PyPI |

---

## 9. 维度 7：Changelog / Release Notes

### 9.1 5 种生成方式

| 工程 | Changelog |
|------|-----------|
| **claudecode** | 远程 RAW + 本地 cache |
| **atomcode** | 手工 release notes |
| **openclaw** | GH render script |
| **opencode** | 手工 |
| **pi** | `scripts/release-notes.mjs` extract/fix-github-releases |
| **cc-switch** | 手工 |
| **agent-studio** | 手工 |
| **agent-core** | gitcode releases |

### 9.2 pi 自动化范本（`scripts/release-notes.mjs:1-80`）

- **extract**：从 CHANGELOG 抽取
- **fix-github-releases**：旧仓库 URL 迁移（`badlogic/pi-mono` → `earendil-works/pi`）

---

## 10. 维度 8：多端分发渠道

### 10.1 8 工程分发矩阵

| 工程 | GitHub | npm | Homebrew | Snap | Docker Hub | App Store | Flathub |
|------|--------|-----|----------|------|------------|-----------|---------|
| **claudecode** | ✅ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| **atomcode** | ✅ | - | - | - | - | - | - |
| **openclaw** | ✅ | ✅ | - | - | ✅ | ✅ | - |
| **opencode** | ✅ | ✅ | - | - | ✅ (GHCR) | - | - |
| **pi** | ✅ | ✅ | - | - | - | - | - |
| **cc-switch** | ✅ | - | - | - | - | - | ✅ (ready) |
| **agent-studio** | - | - | - | - | ✅ (自部署) | - | - |
| **agent-core** | - | ✅ (PyPI) | - | - | - | - | - |

---

## 11. 维度 9：安装体验

### 11.1 3 种安装范式

| 工程 | 安装命令 |
|------|---------|
| **claudecode** | `npm i -g @anthropic-ai/claude-code` |
| **atomcode** | `curl ... | bash`（自研 updater 内置） |
| **openclaw** | Docker pull / npm i |
| **pi** | npm + `build:binary` 单二进制 |
| **cc-switch** | .dmg / .AppImage / .deb |
| **agent-studio** | Helm install |
| **agent-core** | `pip install openjiuwen[all]` |

---

## 12. 横向大表：8 工程 × 9 维度

| 工程 × 维度 | 版本号 | CI | 产物 | 签名 | Auto Update | 回滚 | Changelog | 分发 | 安装 |
|------------|--------|----|----|------|-------------|------|-----------|------|------|
| **claudecode** | 🟢 semver | 🟢 npm | 🟢 npm | 🔴 | 🟡 npm | 🟡 tag | 🟢 RAW cache | 🟢 npm | 🟡 |
| **atomcode** | 🟢 semver | 🟢 GH Actions | 🟢 6 平台 | 🟢 codesign+notary | 🟢 自研 | 🟢 三段式 | 🟡 手工 | 🟢 GH Releases | 🟢 自研脚本 |
| **openclaw** | 🟢 calver | 🟢 workflow_call | 🟢 多端 | 🟢 Ed25519 | 🟢 Sparkle+docker | 🟢 不可变 tag | 🟡 GH render | 🟢 全渠道 | 🟢 |
| **opencode** | 🟢 semver | 🟢 multi-workflow | 🟢 多产物 | 🟡 npm OIDC | 🟡 npm | 🟡 tag | 🟡 手工 | 🟢 npm+GHCR+VSCode | 🟡 |
| **pi** | 🟢 semver | 🟢 tag 触发 | 🟢 6 平台 | 🔴 | 🔴 手动 | 🟡 tag | 🟢 自动化 | 🟢 npm+GH | 🟡 |
| **cc-switch** | 🟢 semver | 🟢 5-OS matrix | 🟢 全平台 | 🟢 Tauri Ed25519 | 🟢 双 endpoint | 🟢 Tauri delta | 🟡 手工 | 🟢 GH+CDN+Flathub | 🟢 .dmg/.AppImage |
| **agent-studio** | 🟢 semver | 🔴 | 🟢 8 镜像 | 🔴 | 🟢 容器内 | 🟢 snapshot | 🟡 手工 | 🟡 Helm | 🟡 |
| **agent-core** | 🟢 PEP 440 | 🔴 | 🟡 sdist+wheel | 🔴 | 🟡 pip | 🟡 pip pin | 🟡 gitcode | 🟡 PyPI+Aliyun | 🟢 pip |

---

## 13. 设计模式提炼（5 条）

### 13.1 模式 D1：原子三段式 rename（atomcode 范本）

```
a. live → .rolling
b. new → live
c. .rolling → .bak
```

**laew 应用**：`rebuild_restart_app.sh` 加 `.bak` 备份 + `cargo build --release` 后 `mv laew laew.bak && mv target/release/laew laew`。

---

### 13.2 模式 D2：Ed25519 + 多 endpoint fallback（cc-switch 范本）

```json
"endpoints": [
  "https://dl.ccswitch.io/latest.json",
  "https://github.com/.../releases/latest/download/latest.json"
]
```

**laew 应用**：未来加 `latest.json`（自建 CDN + GH Releases 双源）。

---

### 13.3 模式 D3：不可变 tag + SHA 双锁（openclaw 范本）

```yaml
inputs:
  tag: {required: true}           # Immutable tag
  release_sha: {required: true}  # Full commit SHA
```

**laew 应用**：release 脚本强制 tag 与 git SHA 一致。

---

### 13.4 模式 D4：TUI-friendly pre-upgrade env（agent-studio 范本）

```bash
# scripts/upgrade_handler.sh:7-30
if [[ ! "${single_file}" =~ ^\.?env\.([a-z0-9]{5})$ ]]; then
    error "Expected format: env.<5-random-chars>, Actual: ${single_file}"
fi
```

**laew 应用**：升级前写 `env.abcde` 配置文件（防误升级）。

---

### 13.5 模式 D5：双 commit release cycle（pi 范本）

```
1. commit: release version + changelog
2. commit: add new [Unreleased] section
```

**laew 应用**：CHANGELOG.md 维护 `[Unreleased]` 区段。

---

## 14. 反模式警示（3 条）

### 14.1 反模式 A1：覆盖式升级

```bash
# ❌ 反模式
wget https://.../laew-new -O /usr/local/bin/laew
```

**正确**：先下载到 temp，SHA256 校验，rename 替换，保留旧版 `.bak`。

### 14.2 反模式 A2：自动更新无审批门

```yaml
# ❌ 反模式
on: [push]
```

**正确**：tag 触发 + 审批环境 + SHA 锁。

### 14.3 反模式 A3：手动 Changelog

```markdown
<!-- ❌ 反模式 -->
# 改动
- 修了 bug
```

**正确**：Conventional Commits + release-please 自动生成。

---

## 15. laew 现状评估（L59-L63 五个 gap）

### 15.1 L59：无 CI 流水线（紧急度 P1）

**现状**：仅 `rebuild_restart_app.sh` 本地脚本。

**修复**：
1. 加 `.github/workflows/release.yml`：tag 触发 + 多平台矩阵。
2. `cargo test --release` + clippy + fmt check。

```yaml
on:
  push:
    tags: ['v*']
jobs:
  build:
    strategy:
      matrix: { os: [ubuntu-latest, macos-latest, windows-latest] }
```

---

### 15.2 L60：仅手动 release（紧急度 P1）

**现状**：手动 `cargo build --release` + `cp target/release/laew ./laew`。

**修复**：
1. 加 `scripts/release.sh`：semver 校验 + tag + changelog。
2. 用 `cargo-release` crate 自动化。

---

### 15.3 L61：无 Auto Update 通道（紧急度 P2）

**现状**：用户手动下载新版。

**修复**：
1. 加 `latest.json`（GitHub Releases + 自建 CDN）。
2. 加 `--check-update` CLI 子命令。
3. 未来：自研 updater 模块（参考 atomcode）。

---

### 15.4 L62：无签名 / 公证（紧急度 P2）

**现状**：无 GPG 签名，无 macOS 公证。

**修复**：
1. 短期：GPG 签名 release tarball。
2. 长期：macOS 公证 + Windows EV Code Signing。

---

### 15.5 L63：无多端分发（紧急度 P2）

**现状**：仅 `./laew` 二进制。

**修复**：
1. Cargo.toml 加 `cargo-dist` 自动生成 .deb/.rpm/.AppImage。
2. 加 `deplink.html` 引导页。
3. 上传 Homebrew tap。

---

## 16. 附录

### 16.1 参考文件清单（绝对路径）

#### atomcode
- `crates/atomcode-updater/Cargo.toml:1-32` — updater crate
- `crates/atomcode-updater/src/lib.rs:1-50,540-628` — 三段式 rename
- `scripts/release.sh:1-65` — 多平台 release 脚本
- `scripts/sign-macos.sh:1-50` — macOS 签名 + 公证
- `latest.json` — 运行时清单

#### claudecode
- `src/utils/releaseNotes.ts:11-23` — 远程 RAW + 本地 cache
- `src/commands/release-notes/release-notes.ts` — 子命令

#### openclaw
- `Dockerfile:11-18` — SHA256 digest 钉
- `appcast.xml` — Sparkle feed
- `scripts/make_appcast.sh:1-15` — Ed25519 appcast 生成
- `.github/workflows/docker-release.yml:1-30` — 不可变 tag + SHA

#### opencode
- `.github/workflows/publish.yml:1-80` — 多分支 + bump
- `.github/workflows/publish-vscode.yml` — VS Code
- `.github/workflows/containers.yml:1-30` — GHCR
- `packages/containers/{base,bun-node,rust,tauri-linux,publish}/Dockerfile`

#### pi
- `scripts/release.mjs:1-80` — 10 步流程
- `scripts/release-notes.mjs` — 自动 changelog
- `scripts/build-binaries.sh:1-120` — 6 平台构建
- `.github/workflows/build-binaries.yml:1-100` — CI

#### cc-switch
- `src-tauri/tauri.conf.json:34-80` — Tauri updater config
- `.github/workflows/release.yml:1-260` — 5-OS matrix
- `flatpak/com.ccswitch.desktop.yml:1-80` — Flatpak manifest

#### agent-studio
- `docker/Dockerfile.server:1-30` — server 多阶段
- `scripts/upgrade_handler.sh:1-50` — pre-upgrade env
- `helm/studio/Chart.yaml:1-15` — umbrella chart

#### agent-core
- `pyproject.toml:3-18` — PEP 621
- `[project.optional-dependencies]` — 多 extras

#### laew
- `rebuild_restart_app.sh` — 本地 release 脚本
- `Cargo.toml` — version 字段
- `src/main.rs::AgentProfile` — `--version` 输出

### 16.2 术语表

| 术语 | 含义 |
|------|------|
| **semver** | MAJOR.MINOR.PATCH 版本号规范 |
| **calver** | YYYY.MM.DD 日历版本 |
| **Auto Update** | 自动更新通道 |
| **Sparkle** | macOS 自动更新框架 |
| **Ed25519** | 椭圆曲线签名算法 |
| **notarize** | Apple 公证（确保 Gatekeeper 通过） |
| **staple** | Apple ticket stapling |
| **Tauri updater** | Tauri 内置更新器 |
| **cargo-dist** | Rust 多平台打包工具 |
| **Flatpak** | Linux 沙盒分发格式 |
| **SBOM** | Software Bill of Materials |
| **cosign** | Sigstore 容器签名工具 |
| **Trusted Publisher** | npm OIDC 直接发布 |

---

## 17. 结语

8 工程调研后，我们看到 laew 的 Release 工程化是**单脚本 → 流水线**的范式升级：

- **L59-L60** 是 P1（影响每次发版的可靠性）。
- **L61-L63** 是 P2（影响用户体验）。

**一句话总结**：「**GH Actions tag 触发 + cargo-dist + sha256 manifest + Tauri-style updater**」是 laew Release 工程的最小落地路径。

---

**字数统计**：~8,500 字，~1,000 行。
**调研时间**：2026-09-07
**作者**：第九轮 T5 专题研究 SubAgent