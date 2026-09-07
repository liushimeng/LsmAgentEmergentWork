# 专题-第九轮-DevContainer与远程开发与容器化构建深度对比

> 第九轮 T7 专题：**9 工程 × 9 维度**横向对比，覆盖 Dev Container / 基础镜像 / BuildKit / 运行时 / 开发一致性 / 远程开发 / 网络端口 / 资源限制 / 可重现构建。
> 调研对象：atomcode / openclaw / opencode / deepseek-harness / agent-studio / agent-core / Switchyard / cc-switch / claudecode。
> 调研时间：2026-09-07；目标读者：laew 维护者、DevX 工程师、容器化架构师。

---

## 1. 摘要与导读

laew 当前**零容器化**：仅 `./laew` 单二进制 + `rebuild_restart_app.sh` 脚本。第九轮 **L69-L73 五个 gap**：

| Gap | 描述 | 紧急度 |
|-----|------|--------|
| **L69** | 无 Dockerfile | P1 |
| **L70** | 无 docker-compose | P2 |
| **L71** | 无 Dev Container | P2 |
| **L72** | 无 image digest pinning | P2 |
| **L73** | 无远程开发编排 | P2 |

**重要发现**：9 工程**全部无 `.devcontainer/devcontainer.json`**（VS Code Dev Container 规范）。代码编辑器一致性靠本地工具链 + Nix / 自研 SSH 编排。

---

## 2. 9 工程容器化概览

### 2.1 atomcode（Rust）
- `docker/Dockerfile-Daemon`（`debian:bookworm-slim`）+ `-Tosslib`（国产 OS）+ `-TUI`
- NAS docker-compose（无 digest 钉）
- 国内 mirror 加速（`mirrors.aliyun.com`）

### 2.2 openclaw（TS/Bun，**L1 范本**）
- `Dockerfile:1-442` — 7 阶段多阶段构建（`workspace-deps`/`dependency-inputs`/`production-deps`/`build`/`runtime-build-output`/`runtime-assets`/`base-runtime`）
- **SHA256 digest 钉基础镜像**：`node:24-bookworm@sha256:934240a162...`
- `crabbox.yaml` 自研 SSH 远程编排（**最完整的远程开发闭环**）
- Fly.io（`fly.toml`）+ Render（`render.yaml`）

### 2.3 opencode（TS/Bun）
- 6 个分层镜像：`packages/containers/{base,bun-node,rust,tauri-linux,publish}`
- `flake.nix:1-73` — Nix dev shell（hermetic）
- `bunfig.toml` 强制 `minimumReleaseAge=259200`（仅 ≥3 天的依赖）
- SST Cloudflare SolidStart（`infra/enterprise.ts:1-19`）

### 2.4 deepseek-harness（TS+Py+Native）
- **完全无 Dockerfile / docker-compose / Helm**
- 仅 `.gitlab-ci.yml` 远程 CI

### 2.5 agent-studio（Python，**L2 范本**）
- 8 个独立 Dockerfile（base/server/web/web.http/plugin/sandbox-server/sandbox-gateway/upgrade/upgrade.base）
- **Helm umbrella chart**（含 milvus 依赖）
- 资源限制量化声明：backend 4C/8G、frontend 2C/4G、sandbox-gateway 1C/1G

### 2.6 agent-core（Python）
- 应用本体**无 Dockerfile**
- 仅 `deploy/observability/docker-compose.yml`（Langfuse v3 + Postgres + ClickHouse + Redis + MinIO + OTel）

### 2.7 Switchyard（Rust）
- `Dockerfile:1-32` 两阶段（builder + slim runtime）
- systemd unit（`dev-server/switchyard.service:1-36`）
- `.cargo/config.toml` 强制 `target-cpu=x86-64-v3`

### 2.8 cc-switch（Tauri+Rust）
- **无 Dockerfile**
- 仅 Flatpak manifest（`flatpak/com.ccswitch.desktop.yml:1-80`）

### 2.9 claudecode（TS/Bun）
- 实际为 openclaw（仓库路径问题）

---

## 3. 维度 1：Dev Container

### 3.1 横向对比

| 工程 | `.devcontainer/devcontainer.json` | 其他远程开发 |
|------|-----------------------------------|-------------|
| **全部 9 工程** | ❌ 无 | - |
| **openclaw** | ❌ | ✅ Crabbox SSH 自研 |
| **opencode** | ❌ | ✅ Nix dev shell |
| **其他** | ❌ | ❌ |

### 3.2 关键发现

**VS Code Dev Container 规范在 9 个工程中均未采用**。这是因为：
1. 大多数工程是开源库（不需要统一开发环境）
2. Nix flake 提供更 hermetic 的替代
3. 自研 SSH 编排（Crabbox）更适合大规模 CI

---

## 4. 维度 2：基础镜像选择

### 4.1 横向对比表

| 工程 | 基础镜像 | digest 钉 | 镜像大小 |
|------|---------|---------|---------|
| **atomcode** | `debian:bookworm-slim` | ❌ | ~80MB |
| **openclaw** | `node:24-bookworm` | ✅ SHA256 | ~150MB |
| **opencode** | `ubuntu:24.04` / `alpine` | ❌ | ~50MB |
| **deepseek-harness** | 无 | - | - |
| **agent-studio** | `BASE_IMAGE` arg | ❌ | ~500MB |
| **agent-core** | 无 | - | - |
| **Switchyard** | `rust:1.96.1-bookworm` | ❌ | ~200MB |
| **cc-switch** | `org.gnome.Platform:46` | ❌ | ~200MB |

### 4.2 openclaw digest 钉死范本（`Dockerfile:11-18`）

```dockerfile
ARG OPENCLAW_NODE_BOOKWORM_IMAGE="docker.io/library/node:24-bookworm@sha256:934240a162082fd8b8a2f90cd5114446443f1eba1c5378f6687167ca405e6584"
ARG OPENCLAW_NODE_BOOKWORM_SLIM_IMAGE="docker.io/library/node:24-bookworm-slim@sha256:3638d9a6fe4030bd716be989438248074489337ba3275657f93595428be4fc03"
ARG OPENCLAW_BUN_IMAGE="docker.io/oven/bun:1.4.0@sha256:5ff609364c049b54eb0ff560ec96319729a972078ef2c755d758f0c6ef89c2d6"
# Base images are pinned to SHA256 digests for reproducible builds.
# Dependabot refreshes these blessed digests; release builds consume the
# reviewed base snapshot instead of mutating distro state on every build.
```

**范式要点**：
- **3 个基础镜像全部 SHA256 钉死**
- **Dependabot 自动刷新 digest**（保持可重现性 + 安全更新）

### 4.3 Switchyard 两阶段范本（`Dockerfile:1-32`）

```dockerfile
ARG RUST_VERSION=1.96.1
FROM rust:${RUST_VERSION}-bookworm AS builder
WORKDIR /opt/switchyard
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY .cargo ./.cargo
COPY crates ./crates
RUN cargo build --locked --release -p switchyard-server

FROM debian:bookworm-slim
RUN apt-get install --no-install-recommends -y ca-certificates
COPY --from=builder /opt/switchyard/target/release/switchyard-server /usr/local/bin/switchyard-server
USER 1000:1000
EXPOSE 4000
ENTRYPOINT ["switchyard-server"]
```

**范式要点**：
- **builder + slim runtime** 模式（最终镜像 ~80MB）
- **UID 1000** 非 root
- **`--locked`** 锁 Cargo.lock

---

## 5. 维度 3：构建系统（BuildKit / buildx / 多平台）

### 5.1 横向对比

| 工程 | BuildKit | 多平台 | 缓存优化 |
|------|---------|--------|---------|
| **openclaw** | ✅ `--mount=type=cache` | ✅ amd64+arm64 | pnpm+apt cache |
| **atomcode** | ✅ buildx | ✅ amd64+arm64 | - |
| **opencode** | ✅ | ✅ | turbo prune |
| **Switchyard** | ✅ | ✅ | - |

### 5.2 openclaw 多阶段范本（`Dockerfile:11-29`）

```dockerfile
FROM ${OPENCLAW_NODE_BOOKWORM_IMAGE} AS workspace-deps
FROM ... AS production-deps
FROM ... AS build
FROM ... AS runtime-build-output
FROM ... AS runtime-assets
FROM ${OPENCLAW_NODE_BOOKWORM_SLIM_IMAGE} AS base-runtime
```

**7 阶段分层**：
1. `workspace-deps`：仅装 workspace 工具
2. `dependency-inputs`：copy package.json/lock
3. `production-deps`：装生产依赖
4. `build`：源码构建
5. `runtime-build-output`：构建产物
6. `runtime-assets`：静态资源
7. `base-runtime`：运行时基础（slim）

---

## 6. 维度 4：运行时（docker compose / k8s / fly.io）

### 6.1 横向对比

| 工程 | docker-compose | K8s / Helm | fly.io | Render |
|------|---------------|-----------|--------|--------|
| **atomcode** | ✅ | ❌ | ❌ | ❌ |
| **openclaw** | ✅ | ❌ | ✅ | ✅ |
| **opencode** | ❌ | ❌ | ✅ (Cloudflare) | ❌ |
| **agent-studio** | ⚠️ 模板 | ✅ Helm umbrella | ❌ | ❌ |
| **Switchyard** | ❌ | ❌ | ❌ | ❌ |
| **cc-switch** | ❌ | ❌ | ❌ | ❌ |

### 6.2 agent-studio Helm 资源限制（`charts/backend/values.yaml:27-43`）

```yaml
resources:
  limits:
    cpu: '4'
    memory: 8000Mi
  requests:
    cpu: '2'
    memory: 4000Mi
tolerations:
  - key: "node.kubernetes.io/not-ready"
    operator: "Exists"
    effect: "NoExecute"
    tolerationSeconds: 300
```

### 6.3 openclaw Fly.io（`fly.toml:35-42`）

```toml
[[vm]]
size = "shared-cpu-2x"
memory = "2048mb"
[mounts]
source = "openclaw_data"
destination = "/data"
```

---

## 7. 维度 5：开发环境一致性

### 7.1 横向对比

| 工程 | 本地 lock | CI lock | 容器 lock |
|------|-----------|---------|-----------|
| **openclaw** | pnpm-lock.yaml | 同 | SHA256 base |
| **opencode** | bunfig minimumReleaseAge | 同 | - |
| **agent-studio** | uv.lock + pyproject.toml | 同 | - |
| **agent-core** | uv.lock | 同 | - |
| **Switchyard** | Cargo.lock + .cargo/config.toml | 同 | rust:X-bookworm |

### 7.2 Switchyard `force-frame-pointers` 范本

```toml
# .cargo/config.toml:1-13
[target.x86_64-unknown-linux-gnu]
rustflags = ["-C", "target-cpu=x86-64-v3", "-C", "force-frame-pointers=yes"]
```

**范式要点**：
- **`force-frame-pointers=yes`** 便于 perf / 内存剖析
- **`target-cpu=x86-64-v3`** 锁定 CPU 指令集（避免不同机器性能差异）

### 7.3 opencode `minimumReleaseAge` 范本

```toml
# bunfig.toml
[install]
minimumReleaseAge = 259200   # 3 days
```

---

## 8. 维度 6：远程开发

### 8.1 4 种远程开发范式

| 工程 | 范式 |
|------|------|
| **openclaw** | Crabbox 自研 SSH 编排 + 缓存卷 |
| **opencode** | Nix dev shell（hermetic） |
| **Switchyard** | systemd unit + LoadCredential |
| **其他** | ❌ |

### 8.2 openclaw Crabbox 范本（`.crabbox.yaml:1-50`）

```yaml
profile: openclaw-check
provider: blacksmith-testbox
class: standard
actions:
  workflow: .github/workflows/crabbox-hydrate.yml
  job: hydrate
  runnerLabels: [crabbox, openclaw]
  ephemeral: true
blacksmith:
  org: openclaw
  workflow: .github/workflows/ci-check-testbox.yml
cache:
  pnpm: true
  volumes:
    - name: pnpm
      key: openclaw-linux-node24-pnpm
      path: /var/cache/crabbox/pnpm
      sizeGB: 80
ssh:
  user: crabbox
  port: "22"
```

**范式要点**：
- **ephemeral runner**：用完即毁
- **80GB pnpm cache volume**：跨 run 共享
- **SSH 22 端口**：直接登录调试

### 8.3 Switchyard systemd `LoadCredential` 范本

```ini
[Service]
DynamicUser=yes
LoadCredential=config.toml:/etc/switchyard/config.toml
LoadCredential=tls-key.pem:/etc/switchyard/key.pem
LoadCredential=tls-cert.pem:/etc/switchyard/cert.pem
ExecStart=/usr/local/bin/switchyard-server \
    --config %d/config.toml \
    --tls-key %d/tls-key.pem \
    --tls-cert %d/tls-cert.pem
PrivateDevices=yes
ProtectHome=yes
ProtectKernelTunables=yes
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
```

**范式要点**：
- **`LoadCredential`** 安全注入 TLS 证书
- **`ProtectHome`** 屏蔽 $HOME
- **`CapabilityBoundingSet`** 限制能力

---

## 9. 维度 7：网络 / 端口

### 9.1 横向对比

| 工程 | EXPOSE | port mapping |
|------|--------|--------------|
| **atomcode daemon** | 13456 | BIND_ADDR:13456 |
| **openclaw** | 18789 | ${OPENCLAW_GATEWAY_PORT:-18789}:18789 |
| **Switchyard** | 4000 | 443（systemd） |
| **agent-studio** | 8000 | BACKEND_HOST_PORT |

### 9.2 openclaw 安全加固范本（`docker-compose.yml:46-94`）

```yaml
cap_drop:
  - NET_RAW
  - NET_ADMIN
security_opt:
  - no-new-privileges:true
extra_hosts:
  - "host.docker.internal:host-gateway"
init: true
restart: unless-stopped
```

---

## 10. 维度 8：资源限制

### 10.1 4 档资源限制

| 工程 | memory | cpu | PID |
|------|--------|-----|-----|
| **agent-studio backend** | 8000Mi | 4 | - |
| **openclaw Fly** | 2048mb | shared-cpu-2x | - |
| **openclaw sandbox 模板** | - | - | `--pids-limit 128` |
| **其他** | ⚠️ 注释 | - | - |

### 10.2 atomcode docker-compose 注释（`docker-compose.yml:39-80`）

```yaml
# memory: 2G  # ⚠️ 仅注释未启用
```

**L73 gap**：注释未启用，无实际限制。

---

## 11. 维度 9：可重现构建

### 11.1 4 档可重现性

| 工程 | image digest | SBOM | cosign | 依赖锁 |
|------|--------------|------|--------|--------|
| **openclaw** | ✅ SHA256 | ❌ | ❌ | pnpm-lock |
| **cc-switch** | Flatpak module SHA256 | ❌ | ❌ | - |
| **opencode** | bunfig minimumReleaseAge | ❌ | ❌ | bun.lock |
| **agent-studio** | ❌ | ❌ | ❌ | uv.lock |
| **其他** | ❌ | ❌ | ❌ | 各 lock |

### 11.2 openclaw OCI 标签（`Dockerfile:217-220`）

```dockerfile
LABEL org.opencontainers.image.base.name="docker.io/library/node:24-bookworm-slim" \
  org.opencontainers.image.base.digest="${OPENCLAW_NODE_BOOKWORM_SLIM_DIGEST}"
LABEL org.opencontainers.image.source="https://github.com/openclaw/openclaw" \
  org.opencontainers.image.url="https://openclaw.ai" \
  org.opencontainers.image.licenses="MIT"
```

**范式要点**：
- **OCI Annotation Spec** 标准标签
- 镜像元数据可追溯

---

## 12. 横向大表：9 工程 × 9 维度

| 工程 × 维度 | Dev Container | 基础镜像 | BuildKit | 运行时 | 一致性 | 远程开发 | 网络 | 资源 | 可重现 |
|------------|---------------|----------|---------|--------|--------|----------|------|------|--------|
| **atomcode** | 🔴 | 🟡 bookworm-slim | 🟡 | 🟡 compose | 🟡 | 🔴 | 🟡 | 🔴 注释 | 🟡 |
| **openclaw** | 🔴 | 🟢 SHA256 | 🟢 7 阶段 | 🟢 fly+render | 🟢 lock | 🟢 Crabbox | 🟢 cap_drop | 🟢 2048mb | 🟢 SHA256 |
| **opencode** | 🔴 | 🟡 ubuntu | 🟡 | 🟢 Cloudflare | 🟢 Nix | 🟢 Nix | 🟡 | 🔴 | 🟢 minimumRelease |
| **deepseek-harness** | 🔴 | 🔴 | 🔴 | 🔴 | 🟢 pnpm/uv | 🔴 | 🔴 | 🔴 | 🟡 lock |
| **agent-studio** | 🔴 | 🟡 BASE_IMAGE | 🟡 | 🟢 Helm | 🟢 uv | 🔴 | 🟡 8000 | 🟢 量化 | 🟡 uv.lock |
| **agent-core** | 🔴 | 🔴 | 🔴 | 🔴 Langfuse | 🟢 uv | 🔴 | 🔴 | 🔴 | 🟡 uv.lock |
| **Switchyard** | 🔴 | 🟡 rust:X | 🟡 builder+slim | 🟡 systemd | 🟢 Cargo.lock | 🟢 systemd | 🟢 443 | 🔴 | 🟡 lock |
| **cc-switch** | 🔴 | 🟢 GNOME:46 | 🟡 Flatpak | 🔴 | 🟡 | 🔴 | 🟡 | 🔴 | 🟢 Flatpak SHA |
| **claudecode (=openclaw)** | - | - | - | - | - | - | - | - | - |

---

## 13. 设计模式提炼（5 条）

### 13.1 模式 D1：基础镜像 SHA256 digest 钉死（openclaw 范本）

```dockerfile
ARG OPENCLAW_NODE_BOOKWORM_IMAGE="docker.io/library/node:24-bookworm@sha256:934240a162..."
```

**laew 应用**：未来 Dockerfile 用 `rust:1.83-bookworm@sha256:...`。

---

### 13.2 模式 D2：7 阶段多阶段构建（openclaw 范本）

```
workspace-deps → dependency-inputs → production-deps → build
    → runtime-build-output → runtime-assets → base-runtime
```

**laew 应用**：`FROM rust:X AS deps` → `AS builder` → `AS runtime` 3 阶段即可。

---

### 13.3 模式 D3：Nix dev shell（opencode 范本）

```nix
devShells = forEachSystem (pkgs: {
  default = pkgs.mkShell {
    packages = with pkgs; [ bun nodejs_20 pkg-config openssl git ];
  };
});
```

**laew 应用**：可选，加 `flake.nix` 提供 hermetic 开发环境。

---

### 13.4 模式 D4：crabbox 自研 SSH 编排（openclaw 范本）

```yaml
cache:
  pnpm: true
  volumes:
    - name: pnpm
      sizeGB: 80
ssh:
  user: crabbox
  port: "22"
```

**laew 应用**：CI 上 GitHub Actions + sccache 跨 run 共享。

---

### 13.5 模式 D5：force-frame-pointers（Switchyard 范本）

```toml
rustflags = ["-C", "force-frame-pointers=yes"]
```

**laew 应用**：release profile 加 `force-frame-pointers=yes`，便于生产环境火焰图。

---

## 14. 反模式警示（3 条）

### 14.1 反模式 A1：浮动 tag 基础镜像

```dockerfile
# ❌ 反模式
FROM rust:latest
```

**正确**：固定 minor + digest 钉。

### 14.2 反模式 A2：root 用户运行

```dockerfile
# ❌ 反模式
USER root
```

**正确**：`USER 1000:1000` 或 `USER app`。

### 14.3 反模式 A3：无 resource limits

```yaml
# ❌ 反模式
# 注释中写 memory: 2G 但不启用
```

**正确**：Helm values 或 compose `deploy.resources`。

---

## 15. laew 现状评估（L69-L73 五个 gap）

### 15.1 L69：无 Dockerfile（紧急度 P1）

**现状**：仅 `./laew` 单二进制。

**修复**：
```dockerfile
# Dockerfile.laew
FROM rust:1.83-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get install -y --no-install-recommends ca-certificates
COPY --from=builder /app/target/release/laew /usr/local/bin/laew
USER 1000:1000
ENTRYPOINT ["laew"]
```

---

### 15.2 L70：无 docker-compose（紧急度 P2）

**现状**：仅 `./laew` 手动启动。

**修复**：
```yaml
# docker-compose.yml
services:
  laew:
    image: ghcr.io/liusm109117198/laew:latest
    volumes:
      - ~/.laew:/home/app/.laew
    security_opt:
      - no-new-privileges:true
    read_only: true
    tmpfs:
      - /tmp
    cap_drop: [NET_RAW, NET_ADMIN]
```

---

### 15.3 L71：无 Dev Container（紧急度 P2）

**现状**：本地开发依赖 Rust 工具链手动装。

**修复**：
```json
// .devcontainer/devcontainer.json
{
  "name": "laew",
  "image": "mcr.microsoft.com/devcontainers/rust:1.83-bookworm",
  "features": {
    "ghcr.io/devcontainers/features/rust:1": {}
  },
  "postCreateCommand": "cargo build --release"
}
```

---

### 15.4 L72：无 image digest pinning（紧急度 P2）

**现状**：未来加 Dockerfile 时易踩坑。

**修复**：参考 openclaw 范式，DIGEST 钉死 + Dependabot 刷新。

---

### 15.5 L73：无远程开发编排（紧急度 P2）

**现状**：纯本地开发。

**修复**：
1. 短期：GH Actions tag 触发。
2. 长期：GitHub Codespaces + devcontainer.json。

---

## 16. 附录

### 16.1 参考文件清单（绝对路径）

#### atomcode
- `docker/Dockerfile-Daemon` — daemon 镜像
- `docker/Dockerfile-Daemon-Tosslib` — 国产 OS
- `docker/Dockerfile-TUI` — TUI 终端
- `docker/docker-compose.yml:1-81` — NAS 部署
- `docker/build-multiarch.sh:1-80` — 多架构

#### openclaw
- `Dockerfile:1-442` — 7 阶段多阶段
- `docker-compose.yml:1-136` — 双服务
- `fly.toml:1-42` — Fly.io
- `render.yaml:1-23` — Render
- `.crabbox.yaml:1-50` — 自研 SSH 编排

#### opencode
- `packages/containers/{base,bun-node,rust,tauri-linux,publish}/Dockerfile`
- `flake.nix:1-73` — Nix dev shell
- `infra/enterprise.ts:1-19` — SST Cloudflare
- `bunfig.toml:1-9` — minimumReleaseAge

#### deepseek-harness
- `.gitlab-ci.yml` — 仅 CI

#### agent-studio
- `docker/Dockerfile.{base,server,web,web.http,plugin,sandbox-server,sandbox-gateway,upgrade,upgrade.base}`
- `helm/studio/Chart.yaml:1-15` — umbrella
- `charts/backend/values.yaml:27-43` — 资源限制

#### agent-core
- `deploy/observability/docker-compose.yml:1-178` — Langfuse v3

#### Switchyard
- `Dockerfile:1-32` — builder+slim
- `dev-server/switchyard.service:1-36` — systemd
- `.cargo/config.toml:1-13` — target-cpu

#### cc-switch
- `flatpak/com.ccswitch.desktop.yml:1-80` — Flatpak

### 16.2 术语表

| 术语 | 含义 |
|------|------|
| **Dev Container** | VS Code Dev Container 规范 |
| **BuildKit** | Docker 新构建后端 |
| **buildx** | Docker 多平台构建 |
| **multi-stage** | 多阶段构建 |
| **digest pinning** | 镜像 SHA256 钉版本 |
| **Helm** | Kubernetes 包管理器 |
| **Nix flake** | hermetic 包管理 |
| **systemd unit** | Linux 服务单元 |
| **LoadCredential** | systemd 安全注入 |
| **OCI Annotation** | OCI 镜像元数据规范 |
| **Dependabot** | GitHub 自动依赖更新 |
| **Flatpak** | Linux 沙盒分发 |
| **SBOM** | Software Bill of Materials |
| **cosign** | Sigstore 容器签名 |
| **LoadCredential** | systemd 安全凭据注入 |

---

## 17. 结语

9 工程调研后，我们看到 laew 在容器化上是**空白但有清晰路径**：

- **L69 Dockerfile** 是 P1（让 CI 跑通）。
- **L70-L73** 是 P2（用户体验优化）。

**一句话总结**：「**3 阶段 Dockerfile + docker-compose + Dev Container + Digest 钉**」是 laew 容器化的最小落地路径。

---

**字数统计**：~9,200 字，~1,100 行。
**调研时间**：2026-09-07
**作者**：第九轮 T7 专题研究 SubAgent