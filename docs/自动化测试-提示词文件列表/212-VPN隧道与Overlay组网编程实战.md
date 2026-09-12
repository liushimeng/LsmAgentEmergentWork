# 212 VPN 隧道与 Overlay 组网编程实战

> 编号段 GX01–GX10 · 聚焦「加密隧道与虚拟组网的工程实现」：tun/tap 虚拟网卡 / UDP 加密隧道 / NAT 类型与打洞 / WireGuard 协议 / 策略路由分流 / Kill Switch 防泄漏 / Mesh Overlay / 流量伪装 / 性能基准 / 跨平台服务化
>
> 与现有维度互补说明：
> - `94-网络协议底层与套接字编程`（CL 维）聚焦**单条 socket 连接与 IO 模型**；本文件聚焦**隧道设备、虚拟网卡与对等组网**这些 socket 之上的结构。
> - `66-跨平台电脑使用与虚拟化实战`（BO 维）聚焦**远程桌面与虚拟机使用**；本文件聚焦**网络传输层的自建加密链路**，是远程桌面所走的路本身。
> - `162-Homelab自托管服务与家庭实验室治理`（FB 维）聚焦**家庭服务器上的服务治理**；本文件聚焦**服务之下的组网与隧道基建**（异地访问家庭网络的那条隧道怎么来）。
> - `10-网络协议与安全基础`（J 维）聚焦**协议概念**；本文件聚焦**组网工程的落地实现与性能调优**。
>
> **本文件独特主题**：/dev/net/tun 与 TUNSETIFF / macOS utun 前缀 4 字节 / wintun / tun-tap 区别 / MTU 1300 分片 / ChaCha20-Poly1305 概念 / NAT 四类型判定 / STUN Binding / UDP 打洞 rendezvous / TURN 中继回退 / WireGuard Noise_IK 1-RTT / AllowedIPs cryptokey routing / ip rule 双路由表 / chnroute 分流 / DNS 泄漏检测 / IPv6 泄漏封禁 / mesh gossip 路由 / DERP 中继 / CGNAT 100.64.0.0/10 / 域前置伪装 / padding / BBR 拥塞控制 / iperf3 基准 / systemd-launchd-NSSM 服务化三平台对照。

---

### GX01 tun/tap 虚拟网卡跨平台编程

- **预期档位**: medium
- **考察维度**: 设备接口 / 帧结构 / 平台差异
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gx01_tun.py`：虚拟网卡讲解器 + 帧解析器：(a) Linux：`open('/dev/net/tun')` + ioctl `TUNSETIFF`（IFF_TUN 无以太头 / IFF_TAP 含以太帧）+ `ip link set up`；(b) macOS：utun 控制 socket `SYSPROTO_CONTROL` + `utun_control` 单元号，读写**前 4 字节协议族头**；(c) Windows：wintun（Ring 缓冲零拷贝）与 tap-windows6 对比；(d) tun 读出的裸 IP 包解析：版本/协议号/TCP-UDP 端口三元组抽取（纯 struct 解析）；(e) 地址配置命令三平台对照（`ip addr add` / `ifconfig` / `netsh interface ip`）；(f) 权限要求（Linux root/CAP_NET_ADMIN、macOS root、Windows 驱动签名）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/gx01_tun.py && python3 tmpPlan/agent-test/gx01_tun.py`，对内嵌的 20 字节 IPv4 头样例字节流解析，断言版本=4、协议号=6(TCP)、源目的 IP 还原正确。
  3. Read 源码，再 Bash 断言：含 TUNSETIFF、含 utun 4 字节头、含 wintun、含三平台地址命令。

### GX02 UDP 加密隧道最小实现

- **预期档位**: hard
- **考察维度**: 包格式 / MTU 分片 / 心跳保活
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gx02_tunnel.py`：教学版 UDP 隧道（python asyncio，单文件含 server/client 两模式）：(a) 握手：client 发 `HELLO` + nonce，server 回 `WELCOME` + 会话 id（简化 Noise 流程讲解：真实实现用 Noise_IK，本教学版 HMAC 握手）；(b) 数据包格式：`magic(2) | session(4) | seq(4) | payload`，seq 单调防重放（滑窗 64）；(c) 加密层抽象：`--mode xor`（教学）与 `--mode chacha`（若装 cryptography 则真加密，否则降级 xor 并警告）；(d) **MTU 1400 分片**：应用层超长包拆片 + 重组缓冲；(e) 心跳 10s + 30s 无响应判定断线重连；(f) `--loopback` 自测模式：同进程内起虚拟网卡输入输出对打。
  2. Bash：`python3 tmpPlan/agent-test/gx02_tunnel.py --loopback --mode xor`，断言 1000 个虚拟包收发 seq 连续、重放窗口丢弃注入的重复包、超长包分片重组后 sha256 一致。
  3. Read 源码，再 Bash 断言：含 seq 重放窗口、含 MTU 分片、含心跳参数。

### GX03 NAT 类型与 UDP 打洞

- **预期档位**: hard
- **考察维度**: STUN / 四类型 / 打洞成功率
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gx03_nat.py`：NAT 穿透模拟器：(a) STUN Binding Request/Response 报文结构（magic cookie 0x2112A442 + 96 位事务 id + XOR-MAPPED-ADDRESS 属性）解析器；(b) **四类型判定流程**：full-cone / restricted-cone / port-restricted / symmetric（两台 STUN 服务器对比出口端口变化）；(c) 打洞模拟引擎：给定双方 NAT 类型输出成功概率矩阵（cone×cone 高 / 含 symmetric 低）与策略（预测端口 / 多端口齐射）；(d) rendezvous 中继服务器职责（交换双方观测地址，不转发数据）；(e) TURN 回退：当打洞失败时 allocation 中继与开销说明；(f) ICE 候选框架概念（host/srflx/relay 三类候选优先级）。
  2. Bash：跑矩阵模拟，断言 `cone-cone` 判定成功、`symmetric-port-restricted` 判定失败并给出 TURN 建议；解析内嵌 STUN 响应样例字节断言映射地址还原正确。
  3. Read 源码，再 Bash 断言：含四类型、含 XOR-MAPPED-ADDRESS、含 ICE 三类候选。

### GX04 WireGuard 协议解析

- **预期档位**: medium
- **考察维度**: Noise_IK / cryptokey routing / roaming
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gx04_wg.py`：WireGuard 协议讲解器 + 配置校验器：(a) **Noise_IK 1-RTT 握手**：Initiation（sender index / ephemeral pubkey / 加密 timestamp）与 Response 双包完成密钥协商，无明文身份泄露；(b) Curve25519 静态/临时密钥对与 presharedkey 的后量子加固作用；(c) Transport Data 包：receiver index + counter + AEAD 密文，anti-replay 滑窗；(d) **cryptokey routing**：AllowedIPs 同时是路由表与访问控制（谁有我的公钥+我允许的网段）；(e) endpoint roaming：对端换地址后收到合法包即更新 endpoint（无重握手）；(f) PersistentKeepalive=25 穿 NAT 保活；(g) 配置校验器：解析 `[Interface]`/`[Peer]` ini，检查 PrivateKey 44 字符 base64、AllowedIPs CIDR 合法、两个 Peer 的 AllowedIPs 重叠检测。
  2. Bash：运行校验器对 3 份内置配置（1 合法 / 1 key 长度错 / 1 网段重叠），断言分别给出通过/报错/警告。
  3. Read 源码，再 Bash 断言：含 Noise_IK、含 AllowedIPs 语义、含 roaming、含 keepalive。

### GX05 策略路由与分流

- **预期档位**: medium
- **考察维度**: 双路由表 / ip rule / 分流列表
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gx05_split.py` + `tmpPlan/agent-test/gx05_split.sh`：(a) Linux 方案：`ip rule add fwmark 0x1 table 100` + `ip route add default dev wg0 table 100`，mangle 表打标与 `wg-quick` 的 fwmark 机制；(b) **chnroute 分流**：从 APNIC/delegated 数据生成 CN CIDR 列表 → 直连路由（不进隧道），境外走隧道，脚本输出路由条数统计；(c) 排除网段（公司内网/组播 224.0.0.0/4/链路本地 169.254.0.0/16）；(d) macOS：`route add -ifscope utun9` 与服务 order；(e) Windows：`route print`、`route add 0.0.0.0 mask 0.0.0.0 <gw> metric 1 if <idx>` 与接口跃点数；(f) DNS 分流：国内域名走 223.5.5.5、其余走隧道内 DoH，防污染。
  2. Bash：`bash tmpPlan/agent-test/gx05_split.sh --dry-run`，断言输出含 fwmark/table 100、含 CN 网段计数、含三平台命令各至少一组。
  3. Read 脚本，再 Bash 断言：含排除网段清单、含 DNS 分流策略。

### GX06 Kill Switch 与泄漏防护

- **预期档位**: medium
- **考察维度**: 防火墙规则 / DNS 泄漏 / 断线阻断
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gx06_killswitch.py`：泄漏防护矩阵生成器：(a) **隧道优先防火墙**：默认 DROP OUTPUT，仅放行 wg0 出站 + 到 endpoint 的 UDP（wg-quick PostUp iptables 规则集逐条讲解：`iptables -A OUTPUT ! -o wg0 -m mark ! --mark 0x1 -j DROP` 族）；(b) **DNS 泄漏检测**：对比直连与隧道下解析同一域名的出口（whoami 服务类思路），系统 DoH（Windows 11 / macOS / systemd-resolved）对分流的影响；(c) **IPv6 泄漏**：隧道只代理 v4 时 v6 裸奔，禁 v6 或代理双栈决策表；(d) 断线守护：探测隧道连通（周期 ping 隧道内网关）失败 → 自动加 DROP 规则 → 恢复后撤销（watchdog 循环伪码）；(e) Windows 防火墙规则绑定特定接口（WFP 概念）；(f) 验证清单：ipleak 类检测项 × 预期结果表。
  2. Bash：运行生成器输出矩阵表，断言含 iptables DROP 规则、含 DNS 检测法、含 IPv6 决策、含 watchdog 循环。
  3. Read 输出，再 Bash 断言：含验证清单且每项有预期结果。

### GX07 Mesh Overlay 与路由分发

- **预期档位**: hard
- **考察维度**: 节点发现 / gossip 路由 / 中继
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gx07_mesh.py`：Mesh 网络模拟器：(a) 节点注册：每个节点（公钥/id + endpoint + 虚拟 IP 从 100.64.0.0/10 CGNAT 段分配）向协调服务器上报，形成成员目录；(b) **gossip 路由同步**：每节点把可达邻居表周期泛洪（push-pull），模拟 20 节点收敛轮次并输出收敛曲线；(c) **最短路径下一跳**：Dijkstra 在延迟权重图上算路由表，逐节点生成转发表；(d) 中继：直连失败（双方 symmetric NAT）时经 DERP 类中继节点转发，吞吐减半的成本说明；(e) 死链清理：探测失败 E 次后摘除并 gossip 撤销；(f) 与 Tailscale/libp2p 的概念对应表。
  2. Bash：运行 20 节点模拟，断言 gossip 在 < log₂(20)+2 轮内收敛、注入 3 条断链后路由表自动改道且无环路（逐跳 TTL 校验）。
  3. Read 源码，再 Bash 断言：含 CGNAT 段、含收敛轮次统计、含中继回退。

### GX08 流量伪装与抗封锁（合规自用场景）

- **预期档位**: medium
- **考察维度**: 域前置 / padding / 中转
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gx08_camouflage.md`：伪装技术综述（**明确限定合规场景：企业出海办公/个人隐私保护，禁止用于违法用途**）：(a) **域前置 Domain Fronting**：TLS SNI 与 HTTP Host 分离原理与 CDN 限制现状；(b) WebSocket over TLS + CDN 中转：流量外表是普通 HTTPS，链路拓扑图；(c) length-padding：固定长度填充破坏流量指纹（包长分布对比图数据）；(d) TLS in TLS 检测对抗：外层包长与握手时序随机化思路；(e) 协议特征治理清单（固定问候、恒定心跳间隔的暴露面）；(f) 审计与合规：企业部署需留会话审计接口、员工知情同意、日志保留策略。
  2. Bash：`grep -c -E "域前置|padding|WebSocket|审计" tmpPlan/agent-test/gx08_camouflage.md` 断言 ≥4 个主题齐全。
  3. Read 文档，再 Bash 断言：含合规限定声明、含企业审计要求。

### GX09 性能基准与传输优化

- **预期档位**: medium
- **考察维度**: iperf3 / BBR / 多路复用
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gx09_bench.sh` + `tmpPlan/agent-test/gx09_bench.md`：(a) 基准脚本：iperf3 双向打流（`-R` 反向）、`-u` UDP 打包率、`-P 4` 多流，输出 JSON 汇总（带宽/重传/抖动）；(b) **BBR 开启**：`sysctl net.ipv4.tcp_congestion_control=bbr`（需内核模块 tcp_bbr，CentOS 7 内核 3.10 不支持 → elrepo 升级内核注记）+ 前后对比实验设计；(c) **TCP vs UDP 隧道**：TCP-over-TCP 队头阻塞放大（ meltdown 现象）实验：注入 1% 丢包对比两种封装吞吐；(d) 多路复用 mux：单连接跑 N 流的权衡（降低握手开销 vs 队头阻塞）；(e) 加密开销：AES-NI 硬件加速 vs ChaCha20（无 AES-NI 的路由器更优）；(f) 缓冲区：UDP socket recvbuf 不足丢包的排查（`netstat -su` 的 RcvbufErrors）。
  2. Bash：`bash tmpPlan/agent-test/gx09_bench.sh --self-test`（无 iperf3 时用内嵌回环样例数据），断言输出含 TCP/UDP/多流三组指标与结论。
  3. Read 文档，再 Bash 断言：含队头阻塞实验设计、含 BBR 与 CentOS 7 内核注记、含 RcvbufErrors 排查。

### GX10 跨平台服务化与运维治理

- **预期档位**: hard
- **考察维度**: 服务封装 / 自愈 / 配置轮换
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/gx10_ops.py`：运维治理方案生成器：(a) **三平台服务封装对照**：Linux systemd unit（Restart=always + WatchdogSec + `PostUp` 钩子）、macOS launchd plist（KeepAlive + RunAtLoad + 网络就绪等待 `NetworkState`）、Windows NSSM 封装（AppExit 重启策略）或原生服务；(b) 自愈链路：连通性探测 → 降级重连（指数退避）→ 多 endpoint 轮换 → 失败告警通知；(c) **密钥轮换**：私钥泄漏应对流程（生成新密钥 → 全 peer 同步 AllowedIPs → 灰度切流 → 撤销旧钥）与季度轮换日历；(d) 配置下发：中心化配置仓库（git）+ 节点拉取校验（签名验证）防中间篡改；(e) 日志与审计：连接事件/流量统计（`wg show` 输出解析成 CSV）留存；(f) 故障 Runbook：三分类（隧道不通/速度劣化/部分网段不可达）× 诊断命令清单。
  2. Bash：运行生成器输出 Runbook，断言含 systemd/launchd/NSSM 三段、含轮换流程五步、含三分类故障表。
  3. Read 输出，再 Bash 断言：含配置签名校验、含 `wg show` 解析示例。
