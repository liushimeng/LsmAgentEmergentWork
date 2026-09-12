# 203 网络流量分析与 NetFlow 工程实战

> 编号段 GO01–GO10 · 聚焦「网络流量的采集、解析、分析与可视化」：抓包与 libpcap / NetFlow/sFlow/IPFIX / 流量特征提取 / 异常检测 / DDoS 检测 / 协议分析 / 带宽计费 / 流量可视化 / 网络遥测
>
> 与现有维度互补说明：
> - `10-网络协议与安全基础`（J 维）聚焦**协议原理与加密**（TCP/TLS/加密算法）；本文件聚焦**流量的采集与分析工程**（抓包/解析/统计），是协议的实际运行观测。
> - `94-网络协议底层与套接字编程`（CL 维）聚焦**套接字编程**（实现 TCP/UDP 通信）；本文件聚焦**流量的被动监听与分析**（不参与通信、只观察），是网络编程的"监控视角"。
> - `78-基础设施可观测性与SRE`（CE 维）聚焦**服务可观测性**（指标/日志/追踪）；本文件聚焦**网络层可观测性**（流量/协议/拓扑），是基础设施观测的网络子层。
> - `132-网络安全审计与合规自动化`（DX 维）聚焦**安全合规检查**；本文件聚焦**流量分析技术本身**（解析/统计/可视化），是安全分析的技术基础。
>
> **本文件独特主题**：libpcap 抓包与 BPF 过滤语法 / pcap 文件格式解析 / NetFlow v5/v9/IPFIX 模板机制 / sFlow 采样 / 五元组流聚合 / Top-N 流统计 / 流量特征（包长分布/到达间隔/流持续时间）/ 异常检测（孤立森林/DBSCAN）/ DDoS 检测（SYN Flood/UDP 反射/CC）/ 协议解码（HTTP/DNS/TLS 握手）/ 带宽计费 95 峰 / 流量热力图与拓扑可视化 / eBPF XDP 高性能采集 / 网络遥测 INT / 流数据管道（Kafka+Flink 实时分析）。

---

### GO01 抓包与 BPF 过滤语法

- **预期档位**: simple
- **考察维度**: libpcap / BPF 编译 / 过滤表达式
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/go01_bpf.py`：BPF 过滤语法生成器（输入过滤意图 JSON），输出：(a) **BPF 基础语法**（type：host/net/port / dir：src/dst / proto：tcp/udp/icmp/http/arp — 示例：`host 192.168.1.1` `port 80` `net 10.0.0.0/8` `src host 1.2.3.4 and dst port 443`）+ (b) **逻辑运算符**（and/or/not + 括号优先级 — `tcp and not port 22` `host 1.1.1.1 or host 2.2.2.2`）+ (c) **高级过滤**（TCP 标志位：`tcp[tcpflags] & tcp-syn != 0` / 包长：`less 60` `greater 500` / 特定字节偏移：`ip[0] & 0xf0 != 0x40`（非 IPv4）/ VLAN：`vlan` + MPLS：`mpls`）+ (d) **编译优化**（BPF 字节码 / 内核态过滤 vs 用户态过滤 / 减少捕获：过滤前置 + 零拷贝 TPACKET_V3）+ (e) **BPF 验证器**（Linux eBPF verifier 安全检查：无循环上限/无未初始化访问/栈深限制 — 传统 cBPF vs eBPF 差异）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/go01_bpf.py && python3 tmpPlan/agent-test/go01_bpf.py > tmpPlan/agent-test/go01_bpf.md`，断言退出码 0。
  3. Bash 断言：含 host/port/net 语法、含 TCP 标志位偏移、含 eBPF verifier、含逻辑运算符示例。

### GO02 pcap 文件解析与流重组

- **预期档位**: medium
- **考察维度**: pcap 头 / 包解析 / TCP 流
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/go02_pcap.py`：pcap 解析器（纯 python 二进制解析），输出：(a) **pcap 文件结构**（全局头 24 字节：magic 0xa1b2c3d4 / version / thiszone / sigfigs / snaplen / linktype + 每包：ts_sec/ts_usec/incl_len/orig_len + 数据 — struct.unpack 解析）+ (b) **包解析**（以太网头 14B → IP 头 20B → TCP/UDP 头 → 载荷 / 协议识别：IP proto 字段 1=ICMP 6=TCP 17=UDP / IP 分片处理：MF 标志 + offset）+ (c) **TCP 流重组**（五元组 = src_ip,src_port,dst_ip,dst_port,proto / 按 seq/ack 排序 / 处理重传与乱序 / 流开始（SYN）到结束（FIN/RST）— 输出每流字节流 + 首包时间 + 持续时间 + 字节数）+ (d) **统计**（总包数 / 总字节 / 协议分布 / Top-10 流（按字节）/ Top-10 IP — 输出 CSV）+ (e) **大文件处理**（逐包流式解析不载入内存 / mmap 内存映射 / 过滤后索引加速 + pcapng 新格式支持）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/go02_pcap.py`，断言退出码 0；Write 最小合法 pcap（全局头 + 1 包）。
  3. Read `tmpPlan/agent-test/go02_pcap.py`，再 Bash 运行：`python3 tmpPlan/agent-test/go02_pcap.py tmpPlan/agent-test/go02_sample.pcap 2>&1 | tee tmpPlan/agent-test/go02_report.txt`，断言含五元组、含流统计、含协议分布。

### GO03 NetFlow / IPFIX 流量聚合

- **预期档位**: medium
- **考察维度**: 模板 / 字段 / 流导出
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/go03_netflow.py`：NetFlow 解析与聚合器（输入五元组包列表 JSON），输出：(a) **NetFlow v5 固定字段**（7 字段：src/dst addr + next hop + input/output if + packets/bytes + start/end time + src/dst port + TCP flags + protocol / tos / src/dst AS — 固定 48 字节/流）+ (b) **NetFlow v9 模板机制**（Template FlowSet 定义字段类型长度 / Options Template / Data FlowSet 引用模板 ID / 模板刷新 — 解决 v5 固定字段不灵活）+ (c) **IPFIX RFC 7011**（信息元素 IANA 编号：8=srcIPv4 12=dstIPv4 4=protocol 7=srcPort 11=dstPort 1/2=octet/packet delta / 21/22 flowEnd/StartTimeMilliseconds / 企业特定 Enterprise Number — 国际化标准）+ (d) **流老化策略**（active timeout 1800s：活跃流每 1800s 导出 / inactive timeout 15s：空闲 15s 老化导出 / 强制老化 FIN/RST — 平衡实时性与导出量）+ (e) **sFlow 采样**（1/N 采样率 + 计数器轮询 / 与 NetFlow 差异：采样 vs 全量聚合 / 适合高速口 10G+）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/go03_netflow.py`，断言退出码 0。
  3. Read `tmpPlan/agent-test/go03_netflow.py`，再 Write `tmpPlan/agent-test/go03_packets.json`：含 20 个包分属 3 个五元组。Bash：运行后断言输出含 v5 字段、含 v9 模板、含老化策略、含 sFlow 采样。

### GO04 流量异常检测算法

- **预期档位**: hard
- **考察维度**: 特征工程 / 孤立森林 / 阈值基线
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/go04_anomaly.py`：流量异常检测器（输入流特征时间序列 JSON），输出：(a) **特征提取**（每时间窗 60s：流数 / 总字节 / 总包数 / 平均流字节 / 新源 IP 数 / 唯一目的 IP 数 / TCP SYN 占比 / ICMP 占比 / 端口熵 — 形成特征向量）+ (b) **基线建模**（滚动 7 天均值 ±3σ / 同比环比：同小时同星期 / 周期性消除：工作日/周末/节假日 — 简单有效）+ (c) **孤立森林 Isolation Forest**（随机切分特征空间 / 异常点少切几次即孤立 / 树数 100 子样本 256 / 适用高维 — 输出异常分数 + top-N 异常流）+ (d) **DBSCAN 密度聚类**（流特征空间聚类 / eps=0.5 minPts=5 / 离群点 = 噪声 = 异常 / 适合发现流量尖峰簇）+ (e) **告警抑制**（告警合并 5min 窗口 / 抑制规则：已确认故障不重复报 / 升级：P1 立即 / P2 10min 观察 / P3 日报 — 告警疲劳管理）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/go04_anomaly.py`，断言退出码 0。
  3. Read `tmpPlan/agent-test/go04_anomaly.py`，再 Write `tmpPlan/agent-test/go04_series.json`：含正常+异常点的流量序列。Bash：运行后断言输出含孤立森林、含 DBSCAN、含基线 3σ、含告警抑制。

### GO05 DDoS 检测与防御策略

- **预期档位**: medium
- **考察维度**: SYN Flood / 反射 / CC / 速率限制
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/go05_ddos.py`：DDoS 检测与防御方案（输入流量统计 JSON），输出：(a) **SYN Flood 检测**（半连接队列满 / SYN/SYN-ACK 比例异常（>5:1）/ SYN Cookie 防御：无状态三次握手 / 阈值：SYN/s > 历史 5 倍触发）+ (b) **UDP 反射放大**（DNS/NTP/SSDP/Memcached 放大系数 30-54000× / 检测：小包请求+大包响应不对称 / 防御：源地址验证 BCP38 + 限速）+ (c) **HTTP CC 攻击**（慢速攻击 Slowloris 长连接 / 高频请求 CC 模拟真人 / 检测：单 IP 请求速率异常 + User-Agent 异常 / 防御：WAF 速率限制 + JS 质询 + 验证码）+ (d) **限速算法**（令牌桶：突发容忍 / 漏桶：平滑固定 / 滑动窗口计数 — 各给实现伪代码 + 参数：桶深/填充率）+ (e) **流量清洗**（云清洗中心引流 BGP 宣告 / GRE 隧道回注干净流量 / 近源清洗 vs 近目的清洗 — 分钟级切换）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/go05_ddos.py && python3 tmpPlan/agent-test/go05_ddos.py > tmpPlan/agent-test/go05_ddos.md`，断言退出码 0。
  3. Bash 断言：含 SYN Cookie、含放大系数、含 CC 检测、含令牌桶/漏桶、含 BGP 引流。

### GO06 协议解码：HTTP / DNS / TLS

- **预期档位**: medium
- **考察维度**: 状态机 / 字段解析 / 语义还原
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/go06_proto.py`：协议解码顾问（输入协议类型 + 字节流描述），输出：(a) **HTTP 解析**（请求行：METHOD URI VERSION / 响应：VERSION STATUS REASON / Headers key: value / Body Content-Length / Transfer-Encoding: chunked — 状态机：REQUEST_LINE → HEADERS → BODY / Keep-Alive 多请求复用连接）+ (b) **DNS 解析**（Header：ID/FLAGS/QDCOUNT/ANCOUNT/NSCOUNT/ARCOUNT / Query：域名标签长度+内容 / Type A=1 AAAA=28 MX=15 CNAME=5 / Response：NAME TYPE CLASS TTL RDLENGTH RDATA / 递归 vs 迭代查询流程）+ (c) **TLS 握手解析**（ClientHello：版本+随机+会话 ID+SNI+密码套件列表 / ServerHello：选定的密码套件+压缩 / Certificate 链 / ServerKeyExchange / 1.3 简化：1-RTT / EncryptedExtensions SNI 加密）+ (g) **gRPC/HTTP2**（二进制帧：HEADERS帧+DATA帧 / HPACK 头部压缩 / 流多路复用 stream / 服务器推送 — 与 HTTP/1.1 解码差异）+ (e) **VoIP 解码**（RTP 头：seq+timestamp+SSRC / 编解码：G.711/G.722/Opus / RTCP 质量统计 — VoMOS 评分估算）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/go06_proto.py && python3 tmpPlan/agent-test/go06_proto.py > tmpPlan/agent-test/go06_proto.md`，断言退出码 0。
  3. Bash 断言：含 HTTP 状态机、含 DNS Type、含 TLS 握手步骤、含 HTTP/2 帧。

### GO07 带宽计费与流量报表

- **预期档位**: medium
- **考察维度**: 95th percentile / 流向 / 对等
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/go07_billing.py`：带宽计费与报表生成器（输入接口流量 JSON），输出：(a) **95th Percentile 算法**（每 5min 采样一次 → 月 8640 点 → 排序 → 取第 95% 位 → 丢弃 5% 突发作为计费带宽 / 实现：heap 快速选择或全排序 / 与按峰值/按平均值计费对比 — 最公平）+ (b) **流向分析**（Transit 过境：源和目的都不是本 AS / Inbound 入站：目的本 AS / Outbound 出站：源本 AS / Peering 对等：与对等 AS 间 — 计费差异：Transit 收费 / Peering 通常免费对等）+ (c) **BGP AS 路径解析**（AS_PATH 属性：如 64501 64502 64503 → 流量穿越 / 对等点 = AS_PATH 首跳改变处 / 计费区间 = 本 AS 与对等 AS 间）+ (d) **报表生成**（日报：Top-10 流 + 协议分布 + 95峰趋势 / 月报：95峰计费值 + 各 Peer 流量 + 增长率 / 输出 CSV + Matplotlib 折线图）+ (e) **成本分摊**（按部门/IP 段/项目 tag 分摊流量成本 / 超套餐计费阶梯 / 流量包/固定带宽混合套餐最优选择 — 成本优化建议）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/go07_billing.py && python3 tmpPlan/agent-test/go07_billing.py > tmpPlan/agent-test/go07_billing.md`，断言退出码 0。
  3. Bash 断言：含 95th percentile 算法、含流向分类 Transit/Inbound/Outbound、含 AS_PATH 解析、含成本分摊。

### GO08 流量可视化与网络拓扑

- **预期档位**: medium
- **考察维度**: 桑基图 / 热力图 / 力导向图
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/go08_viz.py`：网络流量可视化方案（输入流量矩阵 JSON），输出：(a) **桑基图 Sankey**（左节点 = 源 AS/IP / 右节点 = 目的 AS/IP / 流宽 = 流量 / 适合展示流量穿越路径与带宽分配 — D3.js Sankey 配置片段）+ (b) **流量热力图**（矩阵：行=源 IP / 列=目的 IP / 颜色=字节数 / 直方图分布：80% 流量集中在 20% 对 — Plotly heatmap 配置）+ (c) **时序折线图**（5min 粒度流量曲线 / 多接口叠加 / 异常标注垂直线 — 交互式缩放 ECharts 配置）+ (d) **力导向拓扑图**（节点 = 路由器/交换机 / 边 = 链路 / 边宽 = 流量 / 点击节点下钻 — D3 force layout 配置）+ (e) **仪表盘集成**（Grafana + ClickHouse 流量库 / 实时刷新 5s / 告警线叠加 / 钻取：点某条流看 Top-10 源 IP — 监控大屏最佳实践）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/go08_viz.py && python3 tmpPlan/agent-test/go08_viz.py > tmpPlan/agent-test/go08_viz.md`，断言退出码 0。
  3. Bash 断言：含 Sankey/heatmap/拓扑图方案、含 D3/ECharts/Grafana、含钻取下钻。

### GO09 eBPF 与 XDP 高性能流量采集

- **预期档位**: hard
- **考察维度**: eBPF 程序 / XDP 挂载 / BPF maps
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/go09_ebpf.py`：eBPF/XDP 采集方案顾问（输入采集需求 JSON），输出：(a) **eBPF 程序结构**（C 子集 / 挂载点：kprobe/tracepoint/XDP/sockmap / BPF maps：hash/array/perf_event / 用户态 libbpf 加载 — 最小 XDP 程序示例：`SEC("xdp") int xdp_prog(struct xdp_md *ctx) { return XDP_PASS; }`）+ (b) **XDP 高性能**（网卡驱动层处理：包到达即处理 / 不分配 sk_buff / 10Gbps+ 单核 / 动作：XDP_DROP/PASS/TX/REDIRECT — DDoS 在 XDP 层直接 DROP 节约 CPU）+ (c) **BPF maps 设计**（per-CPU array 统计包数 / hash map 存五元组流 / ring buffer 推用户态 / LRU hash 自动淘汰旧流 — 共享数据无锁）+ (d) **BCC vs libbpf CO-RE**（BCC：python 嵌入 C 运行时编译 / libbpf CO-RE：编译一次跨内核运行 BTF 重定位 — 生产环境用 libbpf CO-RE）+ (e) **cilium/Hubble 生态**（基于 eBPF 的 CNI 可观测 / Hubble 流量流图 / 策略执行 — 云原生网络流量分析新范式）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/go09_ebpf.py && python3 tmpPlan/agent-test/go09_ebpf.py > tmpPlan/agent-test/go09_ebpf.md`，断言退出码 0。
  3. Bash 断言：含 XDP_DROP/PASS/REDIRECT、含 BPF maps 类型、含 BCC vs libbpf、含 Cilium/Hubble。

### GO10 流数据实时分析管道

- **预期档位**: medium
- **考察维度**: Kafka / Flink / 窗口 / 延迟
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. Write `tmpPlan/agent-test/go10_pipeline.py`：实时流分析管道架构（输入规模需求 JSON），输出：(a) **采集层**（GoFlow2/采集器 → Kafka topic / 分区按五元组 hash 保序 / Avro 序列化 / 压缩 LZ4 — 100Gbps 采集吞吐）+ (b) **Kafka 设计**（topic 分区 = 消费者数 × 并行度 / 保留 7 天 / 副本因子 3 / 生产者 acks=1 平衡可靠性与延迟 — 流处理 source）+ (c) **Flink 处理**（窗口：滚动 60s / 滑动 60s 步长 10s / 会话 30min 间隔 / 水印 Watermark 处理乱序 2s / 状态后端 RocksDB — 每窗口聚合五元组 Top-N）+ (d) **输出层**（ClickHouse 列存 OLAP 查询 / Elasticsearch 全文检索 / Redis 实时看板 / 告警系统 — 查询 1 亿行 Top-10 < 100ms）+ (e) **SLA 与监控**（端到端延迟 < 10s / 恰好一次语义 / 背压处理 / 自动扩缩容 / Grafana 监控看板）。
  2. Bash：`python3 -m py_compile tmpPlan/agent-test/go10_pipeline.py && python3 tmpPlan/agent-test/go10_pipeline.py > tmpPlan/agent-test/go10_pipeline.md`，断言退出码 0。
  3. Bash 断言：含 Kafka 设计、含 Flink 窗口类型、含 ClickHouse/ES、含 SLA 指标。
