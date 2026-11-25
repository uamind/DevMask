# 带认证中继（Relay）契约冻结

> 文档版本：**relay contract v1.2**（relay 增量冻结；v1.1 = 按 [relay-review-memo.md](relay-review-memo.md) R1–R11 修订，v1.2 = 按 memo §5/§6 R12–R16 修订）
> 状态：**冻结**。本文件是设备端 `device_faker_server` relay 模块、Zygisk 注入改造、双侧 WebUI 与验收脚本的唯一口径；与实现冲突时**按本文件判缺陷**。
> 日期：2026-11-25（v1.1 修订：2026-12-01；v1.2 修订：2026-12-02）
> 上游依据：[authenticated-proxy-plan.md](authenticated-proxy-plan.md)（需求与六条已定默认值）、[relay-review-memo.md](relay-review-memo.md)（R1–R11 评审预审记录）、[egress-contract.md](egress-contract.md)（egress contract v1.2，既有契约）、[../API.md](../API.md) §5.25 / §6.6 / §7 / §9、[../CONFIG.md](../CONFIG.md)「网络出口（egress）」、[../SERVER.md](../SERVER.md) §10
> 实现落点：`server/src/relay/`（新增）、`server/src/api/egress.rs`（扩）、`server/src/egress/`（扩）、`src/egress.rs` + `src/spec.rs`（companion 侧）、`module/uninstall.sh`
> 交互边界：**本文件不引入实现代码**；`iptables` / `curl` / `adb` 命令是契约文本的一部分，验收必须逐条可执行
> 许可：GPL-3.0-or-later，`Copyright (c) 2025-2026 Seyud`，`Device Faker: https://github.com/Seyud/device_faker`

本文件与 [egress-contract.md](egress-contract.md) 的关系：**增量**。egress-contract 的其余条目（revision、0600、`:0` 清除语义、探测契约、注入清单、SQLite v2、job `kind=egress`）全部继续有效；本增量替换/收窄的条目集中在 **§9 与 egress-contract.md 的差异与冲突**，每条都写明「冲突：是/否」与理由。

---

## 阅读约定（本文件的增补术语）

| 术语 | 含义 | 不指 |
| --- | --- | --- |
| **relay / 中继** | 设备内只监听 `127.0.0.1` 的**认证中继**：应用连本地无认证代理，中继替它向上游出示凭据 | 不是 Manager 的 API 透传（`/relay/api/v1/devices/{id}/proxy/...`） |
| **端口表（port table）** | `egress.toml` 里每个需要中继的定义上的 `local_port`，由 server 分配并持久化 | 不是 Android `settings` 里的代理值 |
| **listener** | 一个已绑定的 `127.0.0.1:<local_port>` 监听器（HTTP 代理或 SOCKS5 服务端，由上游 `type` 决定） | 不是防火墙规则 |
| **防火墙链族** | 三条链：入口 `DF_EGRESS_RLY`（分派）→ 可重建的 `DF_EGRESS_RLY_ALLOW`（只有 ACCEPT）→ 静态兜底 `DF_EGRESS_RLY_DENY`（每端口 REJECT） | 不是单链；不碰 INPUT/FORWARD |
| **accept 期 uid 双检** | restricted 端口在 accept 后、读协议字节**之前**再判一次 uid（归属 + 白名单），不符立即 close | 不是 iptables 规则；是规则失效时的第二道防线 |
| **uid 策略** | 某个 listener 允许哪些本机 uid 连接：`all`（设备级端口）或 `restricted`（应用级端口，按 `pkg@uid` 解析） | 不是 Token 作用域 |
| **端口重分配（reallocation）** | 既有 `local_port` 不可用（冲突/越界/被占用/角色变化）时换一个新端口并写回文件 | 不是重新分配上游端口 |
| **重启提示（restart hint）** | 端口值变化后，引用该定义的**应用必须重启**才能拿到新注入值；载体是写响应的 `relay.restartRequiredApps` | 不是设备重启、不是服务重启 |
| **中继生效（relay active）** | `[relay].enabled=true` **且** 该定义带凭据 **且** 该定义有 `local_port` **且** 对应 listener 正在监听 | 不是「代理可用」 |
| **降级（relay config degraded）** | `[relay]` 段的非法值被替换为默认值（不报错、不 500） | 不是 fail-open |

命名规则沿用 [egress-contract.md](egress-contract.md)「阅读约定」：线上 JSON 字段 **lowerCamelCase**；`egress.toml` / `config.toml` 键 **snake_case**；查询参数 **snake_case**；路径参数原文。本增量**不新增**错误码，只新增 5 个扁平警告码（§6.4）。

---

## §0 六条已定默认值的落点（与计划书一致性核对）

[authenticated-proxy-plan.md](authenticated-proxy-plan.md) §8 的六条决策，在本契约中的落地位置：

| # | 决策（计划书 §8） | 本契约落点 | 一致 |
| --- | --- | --- | --- |
| 1 | 设备级带认证代理**也走中继**；relay 绑定失败视为「不启用设备级代理」 | §3.1 规则 3/4、§7.1 | 一致 |
| 2 | uid 防火墙**默认开** | §2.2 `firewall` 默认 `true`、§5 全部 | 一致 |
| 3 | **做**本地 SOCKS5 服务端（P1） | §4.2、§4.4 | 一致 |
| 4 | 上游 TLS（`https://proxy`）**本轮不做** | §4.3 末 | 一致 |
| 5 | 并发默认 **256**、单 uid **64**，可配 | §2.2、§4.6 | 一致 |
| 6 | 「可检出性上升」**写进 CONFIG 已知限制** | §8 全部（含三语文档落点表） | 一致 |

**有意偏差（只有 3 条，全部给理由）**：

| # | 计划书原文 | 本契约 | 理由 |
| --- | --- | --- | --- |
| D1 | §4.1「每个 `[proxies.<name>]` 增加只读字段 `local_port`」 | **只为「需要中继」的定义分配 `local_port`**：`[relay].enabled=true` **且** 该定义带凭据（`username` 或 `password` 非空） | 无凭据代理按 §4.2 根本不走中继；给它分配端口只会多一个监听口（可检出性）与一份死配置。端口池 99 个不小于定义上限 64，容量不是理由 |
| D2 | §5.1「设备级端口：放行全部 uid」未说是否需要防火墙 | 设备级 listener **不依赖** uid 防火墙即可工作（`firewall.applied=false` 时仍监听并照样写 `settings`）；应用级 listener **依赖**防火墙（规则没落地就不监听） | 设备级语义就是「整机代理」，它不需要 uid 限制；反过来把它绑在 iptables 上会让「iptables 挂了 → 整机断网」。应用级必须靠 uid 限制，否则等于把凭据借给全设备 |
| D3 | §4.1「端口表随 egress.toml 一起原子落盘」未定义比较/幂等口径 | 端口表与 `[relay]` 段是 **server 拥有字段**：`PUT /egress` 的幂等比较与 diff **排除**它们；请求里出现 → `422 unknown_field`（§1.8、§6.5） | 客户端从不发送 `local_port`，若不排除，每次 `PUT` 都会 `changed:true`，既有幂等测试（egress-contract §1.4、§1.9）直接失效 |

---

## §1 端口分配

### 1.1 区间与保留

| 常量 | 值 | 说明 |
| --- | --- | --- |
| `RELAY_PORT_MIN` | `17800` | 保留区间下界（含） |
| `RELAY_PORT_MAX` | `17899` | 保留区间上界（含） |
| 区间容量 | `100` | 17800–17899 |
| `[relay].base_port` | 默认 `17800` | 允许范围 **17800..=17899**；设备级 listener 固定占用它 |
| 应用级端口池 | 17800–17899 中去掉 `base_port` 的其余 99 个端口 | 容量 `P = 99` |

- **设备级占 `base_port`**：`[egress].default` 指向的定义若带凭据且 relay 生效，该定义的中继端口就是 `base_port`（`local_port == base_port`）。`base_port` 永远不进入应用级端口池。
- 应用级定义只用池内其余 99 个端口。
- 一个定义**最多一个 listener**；同一端口不会被两个 listener 复用。
- 定义数上限沿用 egress-contract §2.1（不超过 64 条），池容量不小于上限，分配必定可满足（除非 OS 层被外部进程占满，见 §1.5）。

### 1.2 端口表的持久化形态

端口表就是 `egress.toml` 里各定义的 `local_port` 整数键（结构见 §2.1）。没有独立文件、没有数据库、没有内存-only 状态：

| 事实 | 存放位置 |
| --- | --- |
| 哪个定义用哪个端口 | `[proxies.<name>].local_port` |
| 设备级端口 | 该定义的 `local_port` 恰等于 `base_port` |
| 启用开关 | `[relay].enabled`（**只有它**是开关；`local_port` 只是端口表缓存） |
| 监听是否健康 | 只在内存（`GET /egress.relay.listeners[]`），不落盘 |

- 端口表随 `egress.toml` 一起原子落盘（0600，tmp → fsync → rename → fsync(dir)，见 §2.4）。
- `local_port` **只读**：客户端 GET 可见、PUT 不接受（§6.5）。
- `[relay].enabled=false` 时 `local_port` **可以继续存在**但不生效（不监听、不写 `settings` 指向中继、注入侧忽略它）。这样重新打开中继时端口不必重算，避免应用无谓重启。

### 1.3 分配算法（输入 → 输出，可复算）

**输入**：代理名 `name`（§2.5 字符集，1..=64 字符）、`base_port B`、当前端口表里**已被其它定义占用**的端口集合 `TAKEN`。

**步骤**：

1. **保留优先**：若该 `name` 在端口表里已有值 `p`，且满足 `17800 <= p <= 17899`、`p != B`、`p` 不在 `TAKEN` 中，则**直接沿用 `p`**，算法结束（这是跨重启稳定的来源）。
2. **首选下标**：对 `name` 的 **UTF-8 字节** 计算 32 位 FNV-1a：

       h = 2166136261
       对每个字节 b：
           h = ((h XOR b) * 16777619) mod 2^32
       i0 = h mod 99

3. **线性探测**：`k = 0,1,...,98`，`i = (i0 + k) mod 99`，候选端口

       port(i) = 17800 + ((B - 17800 + 1 + i) mod 100)

   - `port(i)` 恒落在 17800..=17899；当 `(1 + i) mod 100 = 0` 时不可达（`i` 最大 98），因此 **`port(i)` 恒不等于 `B`**，无需额外排除。
   - 跨过 17899 后回绕到 17800（例：`B=17850, i0=70` → `17800 + ((51+70) mod 100) = 17821`）。
4. **接受条件**：`port(i)` 不在 `TAKEN` 中 **且** 能立即绑定成 **paused listener**（`bind(127.0.0.1:port)` + `listen(backlog)`，**持有 socket、不 accept**）。第一个成功的端口即为结果——**分配与绑定合并**，不再「探测后关闭再重绑」（消除 TOCTOU，memo R10）。绑定失败（`EADDRINUSE`/`EACCES`/其它）→ 继续 `k+1`。
   - 本轮**本进程自己刚释放**的端口不进入 `TAKEN`，也**不**算 C3「被外部占用」（否则重启自撞会被误判为冲突，memo R6）；真冲突要在本轮再 bind 一次失败才成立（步骤 4 本身就是 bind）。
5. **池耗尽**（99 个候选全部被占/不可绑定）→ 该定义**不分配端口**（不写 `local_port`）、不监听，warning `egress_relay_bind_failed`（`reason="pool_exhausted"`，`port=null`）。

**分配顺序（决定撞车时谁赢）**：同一轮分配严格按**落盘后的书写顺序**遍历定义表——`PUT /egress` 用请求数组顺序，手工编辑用文件顺序。先到先得，后到者从自己的 `i0` 起线性探测。

**复算样例（`B = 17800`，池 = 17801–17899）**：

| 代理名 | `h` | `i0 = h mod 99` | `port(i0)` |
| --- | --- | --- | --- |
| `hk_resi` | `3732955428`（`0xDE806124`） | `48` | **17849** |
| `jp_socks` | `1353994617`（`0x50B45179`） | `30` | **17831** |
| `us_la_01` | `1484863389`（`0x5881379D`） | `9` | **17810** |
| `jp_tokyo_02` | `2664631713`（`0x9ED30DA1`） | `84` | **17885** |

**撞车样例**：`px_12` 与 `px_24` 的 `h mod 99` 都是 `33`（`port(33) = 17834`）。若两者同一轮出现且 `px_12` 书写在前：`px_12` 得到 **17834**；`px_24` 探测 `k=1 → i=34 → port(34)=17835`。**换 `base_port` 复算**：`B=17850`、`hk_resi`（`i0=48`）→ `17800 + ((50+1+48) mod 100) = 17899`。

### 1.4 稳定性与保留规则

| 场景 | 结果 |
| --- | --- |
| 服务重启、定义未变 | 端口不变（步骤 1 保留优先；哈希与书写顺序均不变） |
| 同一 `base_port` 下新增定义 | 既有端口全部不变，新定义探测自己的 `i0` |
| 删掉一个定义再重新加回 | 端口回到原值（哈希确定 + 原端口已空出）；若中间被别的定义占用，则探测下一个 |
| 只改上游 `host`/`port`/凭据值 | 端口不变（定义键没变） |
| 改 `name` | 视为**删除旧定义 + 新增定义**：旧端口释放，新名字重新分配（可能仍是同一端口，也可能不是） |
| 改 `base_port` | 新 `base_port` 上的定义（旧默认）回到池；新默认定义改占新 `base_port`；冲突者重分配（§1.5） |
| 设备级 default 换到另一个带凭据定义 | 新默认定义端口改为 `base_port`（重分配）；旧默认定义回到池（重分配） |

### 1.5 冲突与重分配流程

触发重分配的 5 种冲突：

| # | 冲突 | 检测时点 |
| --- | --- | --- |
| C1 | 文件内两个定义 `local_port` 相同（手工改坏） | 读文件（启动 reconcile / `PUT` / `POST /egress/relay/restart`） |
| C2 | `local_port` 越界（不在 17800–17899）或等于当前 `base_port` | 同上 |
| C3 | 该值已被非本进程占用（`bind` 返回 `EADDRINUSE`/`EACCES`） | 建 listener 时 |
| C4 | 角色变化：成为/不再成为 `[egress].default` | 读文件 |
| C5 | 凭据出现/消失导致「是否需要中继」翻转 | `PUT /egress` 写入后 |

**处理顺序（冻结）**：

1. 按 §1.3 步骤 1 做保留校验；C1 时**文件顺序靠前者保留**，其余进入重分配。
2. **先暂停全部 relay listener 的 accept，再关闭「需要重建」的旧 listener**（端口变化 / uid 集合变化 / 全量重启）——**先关后开**，同端口重建不会自撞 `EADDRINUSE`（memo R6/R7）。
3. 对需要重分配的定义，按 §1.3 步骤 2–4 重新求端口并**直接持有 paused listener**（`TAKEN` = 本轮已确定保留/已分配的端口；本进程刚释放的端口不算外部占用）。
4. **端口表有变化时原子写回 `egress.toml`（0600），且必须在 accept 之前**。写回失败 → 相关定义**不 accept**（socket 关闭），`listeners[].listening=false`、`lastError.code="egress_relay_bind_failed"`、`reason="persist_failed"`（理由见 §7.1：注入侧读文件，文件与真实端口必须一致）。
5. 装规则并按 §5.4 校验；通过后才 accept（生命周期见 §5.4）。
6. 记录端口变化（`from → to`）到本次响应的 `relay.portChanges`（§6.1），并按 §1.7 计算 `restartRequiredApps`。
7. 已释放端口的 DENY 规则在 accept 恢复之后按 spec 删除（§5.4）。
8. 日志一行：`relay port reallocated proxy=<name> from=<p> to=<q> reason=collision|out_of_range|addr_in_use|role_change|credentials_change`，**不含任何凭据**。

**重分配代价（写进文档，不隐藏）**：端口变化意味着引用该定义的应用必须重启（§1.7）；手工把 `local_port` 改成会撞车的值等于主动要求一次重分配。

### 1.6 释放与角色变化

| 事件 | 端口动作 |
| --- | --- |
| 定义被 `PUT /egress` 删除（整体替换后不存在） | 释放其 `local_port`（键随定义一起消失），停 listener、撤规则 |
| 定义凭据被清空（`username`/`password` 都变 `null`） | 释放 `local_port` + 停 listener（无凭据 = 直连上游，不再需要中继） |
| 定义凭据从无到有 | 申请端口（§1.3）+ 建 listener |
| `DELETE /egress` | `[proxies]` 整段删除 → 端口表整体释放；`[relay]` 段**保留**（与 `[probe]` 同级的策略段） |
| `[relay].enabled` 改为 `false` | 不释放、不重算（端口表保留），只停 listener + 撤规则 + `settings` 回上游或 `:0`（§3.1） |
| 服务正常退出 | 不停用端口表（文件不动）；关 listener、撤规则（§5.6） |
| `uninstall.sh` | 文件随后被删，端口表随之消失；规则与 `settings` 由脚本清理（§5.6） |

### 1.7 重启提示（`restartRequiredApps`）

写响应的 `relay.restartRequiredApps` 是**唯一**的重启提示载体（不新增警告码，理由见 §6.4 末）。

冻结规则：

1. **触发**：某定义的 `local_port` 值发生变化（首次分配、重分配、角色变化、凭据翻转后的释放/申请都算）。值没变就不提示（重建 listener 不算变化）。
2. **范围**：`config.toml` 中 `[[apps]].egress` 等于该定义名的全部包名，去重、按包名 ASCII 升序。
3. **不含设备级引用**：`[egress].default` 指向的定义端口变化时，`settings` 对新连接立即生效（既有语义：已建立的连接需重连），**不**要求重启任何应用。
4. **同时被设备级与应用级引用**的定义：只要它在 `[[apps]].egress` 里出现，且端口变化，列入。
5. **响应里的其他位置不改**：`apps[].effective` / `proxy` 仍是配置语义，不掺入「是否需要重启」。
6. 客户端义务（Manager job / WebUI / 人工）：看到非空数组必须提示用户重启对应应用；重启前应用仍用旧端口（旧端口已关闭 → 连接被拒，不是静默直连）。

### 1.8 只读字段边界与幂等比较

**写入路径必须遵守**（否则既有幂等契约被破坏）：

| 项 | 口径 |
| --- | --- |
| `PUT /egress` 请求体出现 `localPort`（camelCase）或 `local_port`（snake_case） | `422 validation_failed`，`details.errors[].code="unknown_field"`（§6.5） |
| `PUT /egress` 请求体出现顶层 `relay` / `relayEnabled` / `relayBasePort` | 同上 `unknown_field`（`[relay]` 本轮不由 API 写入，见 §2.2 末） |
| 幂等比较（egress-contract §1.4 第 1 条） | 归一化后排除 `local_port` 与 `[relay]` 段，只比较客户端拥有的字段 |
| `diff=true` 输出 | **不含** `local_port` 行、**不含** `[relay]` 段（server 拥有字段不参与 diff）；`local_port` 的变更只以 `relay.portChanges` 表达 |
| 备份文件 | 含真实内容（含 `local_port`、含密码），故仍 0600 |
| 响应 `revision` | **写后值**（含本次 `local_port` 变化）。客户端必须用响应里的新 revision 继续；拿旧 revision 再写 → `412`（egress-contract §1.9 不变） |
| `PUT` 整体替换 `proxies` 时同名定义 | `local_port` 保留（server 拥有）；新名字分配；消失的名字释放 |
| `DELETE /egress` | 删除 `[proxies]` 整段 → 端口表消失；`[relay]` / `[probe]` 保留 |

> **副作用必须知道**：server 写 `local_port` 会改变 `egress.toml` 字节 → egress revision 推进（egress-contract §1.2 的算法不变，这是自然结果，不是缺陷）。首次升级后第一次 `GET /egress` 与随后的 `PUT` 之间若发生端口补齐，客户端会拿到一次 `412`——按既有「412 重取一次」处理即可。

---

## §2 `egress.toml`：`[relay]` 段与 `local_port`

### 2.1 完整结构（0600）

    # egress.toml —— 代理定义、探测策略、认证中继端口表。权限 0600。
    # 本文件由 device_faker_server 维护；手工编辑后执行 POST /api/v1/egress/relay/restart 生效
    # （或重启服务）。local_port 是服务端分配字段，手工改会被校验/重分配。
    version = 1

    [relay]
    enabled                  = true     # 中继开关；默认 true；缺省即 true
    base_port                = 17800    # 设备级中继端口；17800..=17899
    firewall                 = true     # uid 防火墙（iptables owner match）；默认 true
    max_connections          = 256      # 全局并发上限；1..=1024
    max_connections_per_uid  = 64       # 单 uid 并发上限；0 = 不限

    [proxies.hk_resi]
    type       = "http"
    host       = "203.0.113.10"
    port       = 8080
    username   = "user"
    password   = "<PASSWORD>"
    local_port = 17800                  # 只读：设备级默认（== base_port），server 写

    [proxies.jp_socks]
    type       = "socks5"
    host       = "198.51.100.20"
    port       = 1080
    username   = "u2"
    password   = "p2"
    local_port = 17831                  # 只读：应用级端口（哈希池）

    [proxies.us_la_01]
    type     = "http"
    host     = "203.0.113.10"
    port     = 1080
    # 无凭据 → 不分配 local_port、不监听（§0 D1）

    [probe]
    url             = "http://checkip.amazonaws.com/"
    timeout_seconds = 5
    fail_open       = true

要点：

- `version` 仍为 `1`（本增量是**可加性**变更；老文件没有 `[relay]` 与 `local_port` 也照常解析）。未来若需要破坏性变更再升 `2`。
- 未知键仍「忽略但保留」+ `unknown_field_preserved`（egress-contract §2.1 不变）。
- 整个 TOML 语法错误 → 仍 `500 internal_error`、`details.op="egress_parse"`（既有修复通道 `If-Match: *` 的 `PUT /egress` 仍然有效：`[relay]` 与 `local_port` 的可加性保证 PUT 能重写 `[proxies]`；**但 `[relay]` 段的语义错误不报 500**，见 §2.5）。

### 2.2 `[relay]` 键表

| 键 | 类型 | 默认 | 允许值 | 非法值的降级（§2.5） |
| --- | --- | --- | --- | --- |
| `enabled` | boolean | `true` | `true` / `false` | 视为 `false`（宁可不中继，也不用未知状态） |
| `base_port` | integer | `17800` | 17800..=17899 | 视为 `17800` |
| `firewall` | boolean | `true` | `true` / `false` | 视为 `true`（决策 #2：默认开） |
| `max_connections` | integer | `256` | 1..=1024 | 视为 `256` |
| `max_connections_per_uid` | integer | `64` | 0..=max_connections（`0` = 不限） | 视为 `64`；若大于 `max_connections` 则取 `max_connections` |

- **`[relay]` 不由 API 写入**（本轮范围）：`PUT /egress` 不读不写这一段，请求体出现 `relay*` → `422 unknown_field`。改这一段 = 编辑文件（0600）+ `POST /egress/relay/restart`（或重启服务）。
- **关闭中继的唯一通道**：编辑 `egress.toml` 把 `[relay].enabled` 设为 `false`，再 `POST /egress/relay/restart`。**没有 API 开关**（memo R8）——`docs/CONFIG.md` 与 `docs/SERVER.md` 必须把这条写成「用户找得到开关」的说明（§8.3）。
- 语义等价：`[relay]` 整段缺失等同于全部默认值（即**中继默认开**）。这是相对 v1.10.0 的**行为变化**，属本增量的目的（带凭据代理从「一定不可用」变成「默认可用」），已写入 §9 与文档收口清单。
- `[relay]` 与 `[probe]` 同级：`DELETE /egress` 不删它。

### 2.3 `local_port` 语义与只读约束

| 项 | 冻结值 |
| --- | --- |
| 类型 | integer，17800..=17899 |
| 谁写 | **只有 server**（分配/重分配/释放） |
| 客户端可见性 | `GET /egress` 与 `GET /egress/proxies` 的 `localPort` 字段只读回显 |
| 客户端写入 | `PUT /egress` 请求体出现 → `422 unknown_field`（§6.5） |
| 手工编辑 | 合法（范围内、唯一、不等于 `base_port`）→ **尊重并保留**；否则重分配（§1.5 C1/C2） |
| 出现条件 | 仅当 `[relay].enabled=true` **且** 该定义带凭据时才有值 |
| 释放条件 | 定义删除 / 凭据清空（§1.6） |
| 文件里保留但 `enabled=false` | 合法状态：端口表缓存，不监听、不生效 |

### 2.4 0600 约束（硬约束，沿用并扩展到新增字段）

| 项 | 冻结值 |
| --- | --- |
| 文件权限 | `egress.toml` **0600 root**；目录 0700 root |
| 创建 | `O_CREAT` + `O_EXCL` 一步 0600（禁止「先 0644 后 chmod」） |
| 落盘 | 临时文件 → `fsync(file)` → `rename` → `fsync(dir)`；`local_port` 的写回走**同一**路径 |
| 读时自愈 | `mode & 0o077 != 0` → `chmod 0600` + 日志 warn + warning `egress_perm_fixed`（既有） |
| 备份 | `backups/egress.toml.<stamp>-<rev8>.bak`，0600，保留最近 10 份（既有） |
| 导出 | **不进** `GET /config?format=toml`、不进 WebUI 导出、不进任何 API 明文；API 只回 `hasCredentials` 与 `localPort` |
| 日志 | 不记录文件内容；只允许记录「分配/重分配了哪个代理的哪个端口」 |
| 权限边界 | `[relay]` 与 `local_port` 不改变敏感级别：它们与密码同文件、同 0600 |

### 2.5 解析与降级规则（`[relay]` 段）

| 情形 | 行为 |
| --- | --- |
| `[relay]` 段缺失 | 全部取默认值（`enabled=true`、`base_port=17800`、`firewall=true`、256/64） |
| 单个键类型/范围非法 | 该键取默认值（降级），日志一行 `relay config degraded key=<key>`；**不**返回 500、不写文件、不新增警告码；实际生效值由 `GET /egress.relay` 暴露（`enabled` / `basePort` / `failOpen` / `limits` 是**实际值**，不是文件原文） |
| 段名拼错（`[relayy]`） | 按未知键「忽略但保留」+ `unknown_field_preserved`（`details.fields` 含 `relayy`） |
| 整个文件语法错误 | `500 internal_error`、`details.op="egress_parse"`（既有） |

**为什么语义错误不报 500**：`PUT /egress` 不写 `[relay]`，一旦报 500 就没有 API 修复通道（`GET` 500 就拿不到 revision；`DELETE` 也会 500）。降级 + 实际值可见是唯一「不砖」的口径。

### 2.6 与 `config.toml` 的边界

`config.toml` 保持 0644（可分享、可导出），因此：

- 出现 `[relay]` 段 → `422 unknown_field`（中继端口与开关属 `egress.toml`）。
- `[egress]` 段仍只允许 `default` 一个键（egress-contract §2.3 不变）；出现 `local_port` / `relay` 等键 → `422 unknown_field`。
- `[[apps]]` 记录里出现 `local_port` → `422 unknown_field`。

---

## §3 生效规则

三条优先级的**总纲**（设备级与应用级各自展开，见 §3.1 / §3.2）：

1. `__DIRECT__`（仅应用级）→ **强制直连**，任何 relay 状态都不改变它。
2. 命中定义 **带凭据** 且 relay 生效 → **走中继**（地址是 `127.0.0.1:<local_port>`）。
3. 命中定义 **无凭据**（或 relay 未生效）→ **直连上游**（地址是上游 `host:port`）。

### 3.1 设备级（写 `settings global http_proxy`）

按**优先级**判定（服务端行为，结果体现在 `settings`、`applied/readback`、`warnings`）：

1. `[egress].default` 为空 → 不启用代理：`settings put global http_proxy :0`。无中继、无 listener。
2. `default` 指向的定义不存在 → 同上清直连 + `egress_proxy_not_found`（既有，`scope:"device"`）。
3. 命中定义 **无凭据** → **直连上游**（不经中继）：写上游 `host:port`（v1.10.0 行为）。
4. 命中定义 **有凭据** 且 `[relay].enabled=true` 且该定义 listener 正常 → 写 **`127.0.0.1:<local_port>`**（即 `127.0.0.1:base_port`）。不产生任何凭据相关 warning。
5. 命中定义 **有凭据** 且 `[relay].enabled=false` → 写上游 `host:port` + `egress_credentials_ignored`（既有语义：设备级不承载认证）。
6. 命中定义 **有凭据** 且 `[relay].enabled=true` 但该端口**绑定/持久化失败** → **不写**（清成 `:0`）+ `egress_relay_bind_failed`（决策 #1：绑定失败 = 不启用设备级代理，不做半死不活）。

补充（冻结）：

- 期望值的比较与读回规则不变（egress-contract §3.3 真值表）：只是期望值可能是 `127.0.0.1:17800`，`readback.settingsValue` 会显示它，`applied` 照常计算。
- **`exclusionList` 不得包含回环**：中继生效时（规则 4），`exclusionList` 里出现 `127.0.0.1` / `localhost` / `::1`（大小写不敏感、含带端口写法）→ `422 invalid_value`（`path="exclusionList[i]"`）。理由：排除回环会让系统代理对回环目标失效 → 流量静默打回直连（泄漏），且用户完全看不出来。
- 清除仍只能用 `settings put global http_proxy :0`（`settings delete` 无效，egress-contract §3.2 不变）。
- 设备级 listener 的 uid 策略恒为 `all`（§5.2），不做 `pkg@uid` 解析。
- **设备级只对 `http` 上游建中继 listener**：`[egress].default` 指向 `type="socks5"` 的定义时，`settings global http_proxy` 无法表达 SOCKS 语义，**不会**在 `base_port` 上建 device listener（`relay.listeners[]` 里没有 `scope="device"` 条目，`proxies[].relayActive=false`）。`PUT /egress` 按**合并后的有效 default** 校验：指向非 http 定义 → `422 validation_failed`（`errors[].code="invalid_egress_default"`、`path="default"`），与 `[relay].enabled` / 是否带凭据**均无关**。**已存在的旧配置**按 `[relay].enabled` 分流：`true` → `settings` 写黑洞 `127.0.0.1:9`（fail-closed：不把 HTTP 流量送到 SOCKS 端口、也不泄漏直连），`readback.settingsValue` 回显 `127.0.0.1:9`；`false` → 按「未配置」处理（`settings :0` 直连），**不写黑洞**。两种情形都不会建 SOCKS5 listener，且**都会**在 `warnings` 里给 `egress_relay_device_socks5_unsupported`（配置提醒，与 `enabled` 无关）。**用法**：socks5 定义只能通过 `[[apps]].egress` 引用（app scope + uid 白名单）；默认出口请选 http 定义。

### 3.2 应用级（Zygisk 注入，方案 B）

注入发生在 `app_specialize`，生效时机是**应用进程启动**（既有语义）。companion 只依据**静态事实**（`egress.toml` 的 `[relay].enabled` 与 `local_port`）决定注入地址，**不询问 server 运行时健康**（无 IPC，后果见 §7.2）。

按**优先级**判定：

1. `[[apps]].egress == "__DIRECT__"` → 强制直连（direct selector + 清空全部代理属性），与 relay 无关（既有 §5.2 不变；设备级指向中继时也必须能拉回直连）。
2. `[[apps]].egress` 键不存在 / 空串 → **不注入**（继承设备级；设备级指向中继则自动继承，无需重复注入——既有行为，避免每个进程起重放线程）。
3. `[[apps]].egress` 指向的定义不存在 → 不注入（既有；server 侧报 `egress_proxy_not_found`）。
4. 命中定义 **无凭据** → **直连上游**（不经中继）：注入上游 `host:port`（`http` 或 `socks5`，按 type 走属性互斥表，既有 §5.3 不变）。
5. 命中定义 **有凭据** 且 **中继明确未启用**（`[relay].enabled=false`，**或** 文件未迁移：无 `[relay]` 键且没有任何 `local_port`）→ **回退到上游**：注入上游 `host:port`，**不注入凭据载体**（v1.2 起一律不写，§3.3、§9.2 B13）；此路径产生 `egress_auth_unsupported`（该代理的认证在本状态下不可用）。
6. 命中定义 **有凭据** 且 relay 生效（`[relay].enabled=true` **且** 有 `local_port`）→ 注入 **`127.0.0.1:<local_port>`**，**不注入任何凭据载体**（§3.3）。
7. 命中定义 **有凭据**、`[relay].enabled=true` 且文件**已迁移**（有 `[relay]` 键或存在任何 `local_port`），但该定义**没有** `local_port`（池耗尽 / 写回失败 / 手改坏）→ **fail-closed**：注入 `127.0.0.1:9`（§3.3 哨兵端口），**不回退上游、不静默直连、不写凭据**。v1.2 的收窄：只要中继是「开着的」，缺端口就是故障，不是回退理由（R13）。

**关键不变量（评审重点）**：

- 注入目标要么是「真实上游」，要么是「本地中继」，**永不**是第三条路；relay 生效时应用侧看到的地址必然是 `127.0.0.1`。
- 应用级 relay 失败**不是 fail-open**：不改成直连、不改设备级 `settings`（§7.4）。已注入旧值的应用会得到「连接被拒」（安全：不泄漏、不静默换出口）。
- 走 `http` 定义时**只**设 `http(s).proxy*`，走 `socks5` 定义时**只**设 `socksProxy*`（属性互斥表不变，egress-contract §5.3）。
- **fail-closed 优先于「不注入」**：只要 spec 声明了 `relay=true`，任何不能安全指向中继的情况都必须注入 `127.0.0.1:9`（经 `injection_plan`、arm replay），绝不返回「未配置」——返回未配置会让应用继承设备级代理或直连（R12）。
- **覆盖边界**：以上只控制 **Java 层**（6 个属性 + `ProxySelector`）；native 栈与 WebView 本来就不受控（§3.4），fail-closed 不承诺它们。

### 3.3 companion → spec 的 `[egress_proxy]` 字段（模块侧读它）

`GetSpec` 下发的 TOML 里 `[egress_proxy]` 小节是 public 契约（模块不读 `egress.toml`）：

| 键 | relay 生效时 | relay 未生效时（既有） |
| --- | --- | --- |
| `type` | `"http"` / `"socks5"` | 同 |
| `host` | **`"127.0.0.1"`**（fail-closed 时同样是 `127.0.0.1`） | 上游 host |
| `port` | **`local_port`**（17800–17899）；**`9` = fail-closed 哨兵** | 上游 port |
| `username` / `password` | **不写**（缺省） | **也不写**（v1.2 起一律不写，§9.2 B13） |
| `relay` | **`true`** | 不写（缺省 `false`） |
| `type = "direct"` | 不变（`__DIRECT__` 专用） | 同 |

模块侧（zygote 域）冻结规则：

1. **输入校验**（只对 `relay=true` 的段）：`host` 必须是 `"127.0.0.1"`、`port` 属于 {9} 并上 [17800,17899]、`type` 属于 {`http`, `socks5`}。`port=9` 是**唯一的 fail-closed 哨兵**（常量 `RELAY_FAIL_CLOSED_PORT = 9`，并断言不在 17800..=17899）。
2. **非法目标 = fail-closed，不是「未配置」**（R12）：`relay=true` 但 host 非回环 / port 不在池内且非 9 / type 非法 → **仍然生成一个可注入的代理**：`host="127.0.0.1"`、`port=9`、`relay=true`、无凭据；**必须经过 `injection_plan`**（从而 arm replay），绝不返回「无计划」、绝不回退上游。日志：`[egress] relay fail-closed package=<pkg> reason=invalid_target|port_missing creds=0|1`（`creds` 只表示 spec 里是否带过凭据，**不打印值**）。
3. **唯一例外：仅多带凭据**（R12）：host/port 合法但 spec 里出现 `username`/`password` → **剥掉凭据、照常注入中继目标**（这是修复，不是降级）；日志 `[egress] relay creds stripped package=<pkg>`。
4. **`type="direct"` + `relay=true`（自相矛盾）→ 按「主动直连」处理**：清 6 个代理属性 + `ProxySelector` 返回 `NO_PROXY`、arm replay；**不**返回「未配置」（那会继承设备级代理，违背 `__DIRECT__` 语义）。
5. `relay=true` 时**永不写凭据载体**（`java.net.socks.username`/`password`、`Authenticator`）——本地中继不需要认证，写进去只会把凭据暴露在应用进程里。
6. **哨兵注入结果**：6 个属性按 type 指向 `127.0.0.1:9`，另一协议属性必须清空；`ProxySelector` 指向 `127.0.0.1:9`；日志 `[egress] injecting relay package=<pkg> type=<t> port=9 blocked=true`。连接立刻 `ECONNREFUSED`（内核拒绝，不是 DROP），表现为**断网**而不是换出口。
7. 其余注入清单（6 个属性、`ProxySelector`、后置重放时间表、失败不阻断启动）**逐条不变**（egress-contract §5.1 / §5.4 / §5.5）；旧分支「`injection_plan` 返回 `None` → 日志 `error=missing endpoint`」**删除**——检测到非法 relay 视图时不再有「无计划」路径。
8. **残留风险（必须写进文档）**：注入只覆盖 Java 层；native 栈与 WebView 不受控（§3.4），fail-closed 对它们不生效；`port=9` 依赖内核回 `ECONNREFUSED`，验收必须先确认设备上 9 端口无监听（§5.7 第 5 组）。

### 3.4 `__DIRECT__`、WebView、排除列表的边界

| 项 | 行为 |
| --- | --- |
| `__DIRECT__` | 永远直连；设备级指向中继也不改变（§5.2 主动直连不变） |
| WebView | 只看系统代理 → 设备级中继对 WebView 生效；应用级中继对 WebView 无效（既有 §5.6） |
| native 栈 | 同既有：跟随系统代理的栈受设备级中继影响，其余不受 |
| `exclusionList` | 与中继无关（系统代理排除表），但禁止排除回环（§3.1） |
| UDP / QUIC | 不经中继（中继是 TCP 代理）；既有限制不变 |

### 3.5 探测（`POST /egress/probe`）：`via=upstream` 与 `via=relay`

探测有**两条路径**，由请求字段 `via` 选择（memo R2）：

| 请求 `via` | 含义 | 走哪里 | 缺省 |
| --- | --- | --- | --- |
| `"upstream"` | **直连上游**（不经中继） | 服务进程按定义直接连上游、带凭据（与 v1.10.0 同一实现） | 是（缺省值，逐字节兼容 v1.10.0） |
| `"relay"` | **经设备内中继** | 按 `package`（或设备级默认）选中该定义的中继端口，以**无凭据**代理通道连 `127.0.0.1:<local_port>`，完整做一次往返 | 否 |

`via="relay"` 一次覆盖四件事：「本地端口在监听 + 中继能转发 + 上游认证通过 + 目标站点可达」——这正是「探测绿但应用不通」的补丁。

规范（冻结）：

1. **判定顺序（冻结，captain 2026-12 口径）**：先选链路（给了 `package` → 该应用 `[[apps]].egress` 指向的定义；未给 → `[egress].default`），然后**严格按序**判定：
   - ① `[relay].enabled=false` **或** 该定义无凭据（`hasCredentials=false`）→ `409 egress.not_configured`，`details={scope, package?, via:"relay"}`（`via` 是**新增**字段，只在请求 `via="relay"` 时出现）。
   - ② 有凭据且 `enabled=true`，但**没有可用的监听端口**（无 `local_port`、端口不在端口表、或 `listeners[].listening=false`）→ `502 egress.probe_failed`、`details.reason="relay_not_listening"`、`relayPort` 为 `null`（无端口）或实际端口（端口在但未监听）。
   - ③ 端口存在且在监听 → 连 `127.0.0.1:<local_port>` 做一次往返；连接/握手失败 → `reason="relay_connect"` 或 `timeout`。
   **不要把 ② 归进 409**：`enabled` 与凭据回答「有没有配置中继」，`listening` 回答「中继此刻是否可用」，分属 409 / 502。t11 的 `docs/API.md` 与 `manager/docs` 按同一顺序写。
2. **通道**：`Channel::Proxy{host:"127.0.0.1", port:local_port, kind}`，**不带任何凭据**——凭据由中继注入，探测侧再带一份只会掩盖「中继没带凭据」的真实故障。目标 URL、超时、响应体解析与 v1.10.0 **完全一致**（egress-contract §4.2–§4.3）。
3. **响应**：
   - `via` 值域扩展为 `"proxy" | "direct" | "relay"`（新增 `"relay"`）；`via="relay"` 的探测恒回 `"relay"`，`via="upstream"` 沿用既有 `"proxy"` / `"direct"` 语义。
   - 新增 `relayPort: integer | null`：`via="relay"` 时为实际使用的端口，否则 `null`。
   - `upstream` / `proxies[]` 的 `host`/`port` **永远上游真实地址**；中继地址只出现在 `relay.listeners[].port`、`proxies[].localPort` 与本次响应的 `relayPort`。
   - `scope:"app"` 的 `approximate:true` 与 `warnings:[{code:"egress_probe_approximate"}]` **不变**（无论 `via` 取值）：探测仍在服务进程内、以 uid 0 发起。
4. **失败**：仍走 `502 egress.probe_failed`（不新增对公错误码）；`details` 追加 `relayPort?`；`reason` 枚举**只增不改**，新增两项：

| reason | 触发（仅 `via="relay"`） |
| --- | --- |
| `relay_not_listening` | 有凭据且 `enabled=true`，但无可用监听端口：无 `local_port`（fail-closed）/ 端口不在端口表 / `listeners[].listening=false`（uid 解析失败、防火墙失败、bind 失败） |
| `relay_connect` | 连 `127.0.0.1:<local_port>` 被拒或握手失败（listener 刚退出、并发超限等） |

其余 reason 沿用 `connect` / `dns` / `timeout` / `tls` / `http_status` / `invalid_response`；`via="relay"` 时它们描述的是**中继到上游**那一段（中继把上游失败映射成 `502`，探测侧多数会看到 `http_status` / `invalid_response`，两种都允许）。
5. **fail-open 门控（安全关键，memo R2/R9）**：只有 **`via="upstream"` 且 `reason ∈ {connect, dns, timeout, tls}`** 才允许按既有三时点规则触发 fail-open（清 `settings` 回直连）。**`via="relay"` 的失败一律不动 `settings`**——即使 `reason="timeout"` 也不触发。否则「中继死 → 探测红 → 清系统代理 → 全设备静默直连」，把 fail-closed 变成 IP 泄漏。`via="relay"` 的失败只回错误信封（`scope:"app"` 时仍带既有 `egress_probe_scope_app` 警告语义），**不**产生 §6.4 的 relay 运行态警告。
6. **能力边界（必须写进文档）**：`via="relay"` 由 root（uid 0）发起，而 `DF_EGRESS_RLY_ALLOW` 对 uid 0 永远放行（§5.2）。因此它证明的是「**中继数据面可用**」，**不证明**「目标应用 uid 能连」——后者只能由 t9 在应用 uid 下实测（§5.7 第 2 组、§10.4）。

---

## §4 数据面（本地代理 ↔ 上游）

### 4.1 本地 HTTP 代理（`type="http"` 的上游）

- 监听：**只 bind IPv4 字面量 `127.0.0.1:<local_port>`**（禁止 `0.0.0.0` / `::` / 主机名；bind 后 `getsockname` 硬断言见 §5.1），`SO_REUSEADDR`（只为同端口重建，**不用** `SO_REUSEPORT`），`TCP_NODELAY`，backlog 128。
- 只处理 **HTTP/1.1**；请求头区（到空行为止）上限 **32 KiB**，超过 → `400` + `Connection: close`。请求体不整块缓冲，按流转发。
- 支持两种请求形态：
  1. **CONNECT**：`CONNECT <host>:<port> HTTP/1.1`（authority-form）。缺端口 / 非法 authority → `400`。
  2. **absolute-URI**：`<METHOD> http://<host>[:port]/<path> HTTP/1.1`。其它 scheme（`https://` 直连、`ftp://`）→ `400`。
- 拒绝：origin-form（`GET /path`）、`HTTP/1.0`、`OPTIONS *`、缺 `Host`、`Host` 与目标不一致（防请求走私）→ `400` + close。
- 不预读、不解析 CONNECT 之后的任何字节：握手成功后是**纯字节隧道**（不做 TLS 终止，HTTPS 由应用端到端加密）。
- 不做连接池 / keep-alive 复用：每个客户端连接处理一个请求-响应周期（绝对 URI）或一条隧道（CONNECT）后关闭；响应带 `Connection: close`。
- 上游响应头同样 32 KiB 上限，超过 → `502`。

### 4.2 本地 SOCKS5 服务端（`type="socks5"` 的上游）

- 监听与 TCP 选项同上。**本地不做认证**（应用不需要懂认证）。
- 方法协商：`VER=0x05`、`NMETHODS>=1`；方法列表含 `0x00` → 回 `05 00`；不含 `0x00`（例如只报 `0x02`）→ 回 `05 FF` 并关闭。**只接受 `0x00`**。
- 请求：`VER=0x05`、`CMD=0x01(CONNECT)`、`RSV=0x00`、`ATYP` 属于 {`0x01` IPv4, `0x03` 域名, `0x04` IPv6}、`ADDR`、`PORT`。
  - `CMD != 0x01`（BIND / UDP ASSOCIATE）→ `REP=0x07`（command not supported）。
  - `RSV != 0x00` / 长度为 0 的域名 / `PORT=0` → `REP=0x01`。
  - `ATYP` 不支持 → `REP=0x08`。
- **远程 DNS**：`ATYP=0x03` 的域名**原样**转给上游解析，中继本地**不做任何 DNS 解析**（不泄漏、不说谎）。
- 回复：成功 `REP=0x00` + `BND.ADDR/BND.PORT`（取本地到上游的 socket 地址，端口可为 0）；失败按 §4.7 映射。

### 4.3 上游 HTTP（`type="http"` + 凭据）

- 中继到上游建**明文 TCP**（本轮**不做**代理自身 TLS：不接受 `https://proxy` 形态，`egress.toml` 的 `type` 仍只有 `http` / `socks5`，出现 `https` → `422 invalid_egress_proxy`）。注意：**「不做上游 TLS」不等于不转发 HTTPS 流量**——CONNECT 之后是纯字节隧道，应用与目标站点仍端到端 TLS。
- CONNECT 转发（只发三行头，最小化泄漏面）：

      CONNECT <host>:<port> HTTP/1.1
      Host: <host>:<port>
      Proxy-Authorization: Basic <base64>
      （空行）

- absolute-URI 转发：方法/URI/版本原样 + 客户端头（剥离后，§4.5）+ 注入的 `Proxy-Authorization`。
- 上游响应：状态行与头原样回传（剥离 hop-by-hop），`2xx` 的 CONNECT 之后进入双向隧道；非 `2xx` 原样回传后关闭（`407` 因此能被应用看到——验收项）。
- 上游不可达 / 握手超时 / 响应非法 → `502`（§4.7）。

### 4.4 上游 SOCKS5（`type="socks5"` + 凭据，RFC 1929）

- 有凭据时问候**只报** `0x02`：`05 01 02`；上游必须选 `0x02`，否则（含 `0xFF`）失败。
- RFC 1929 子协商：`01 ULEN UNAME PLEN PASSWD` → 回 `01 STATUS`；`STATUS != 0x00` → 失败（映射见 §4.7），日志 `reason=auth_failed`，**不记**用户名/密码。
- 认证成功后，把客户端的 CONNECT 请求（同 `ATYP/ADDR/PORT`）原样发给上游，读回复并把上游 `REP` 原样回给客户端；`REP=0x00` 后进入双向隧道。
- 上游非 `0x00` 的 `REP` 不代表中继故障 → 原样转发，不产生 warning。
- 无凭据定义不会走 relay（§3.2 规则 4），因此 relay 只在「有凭据」时做 SOCKS5 客户端。

### 4.5 头处理与注入规则（冻结）

| 规则 | 内容 |
| --- | --- |
| **覆盖而非叠加** | 客户端自带的 `Proxy-Authorization`（大小写不敏感、可多条）**全部删除**，由中继插入**恰好一条** `Proxy-Authorization: Basic <base64>` |
| base64 口径 | `base64( username : password )`（中间一个冒号），UTF-8 字节，标准字母表、带 `=` 填充、不换行；`username`/`password` 为空串按空串参与拼接（RFC 7617 允许空 user-id） |
| 头名大小写 | 输出固定写 `Proxy-Authorization`（RFC 9110 大小写不敏感；固定写法便于抓包核对） |
| 剥离（两侧） | `Proxy-Authorization`、`Proxy-Connection`、`Connection`、`Keep-Alive`、`TE`、`Trailer`、`Upgrade`，以及 `Connection` 头里点名的扩展头 |
| 不剥离 | `Host`（必须与目标一致）、`User-Agent`、`Accept*`、`Cookie`、`Range` 等端到端头 |
| 不修改 | 不重写 `Host`、不改写 URI path、不加 `X-Forwarded-*`、不加 `Via` |
| 顺序 | 保留客户端原有头的相对顺序；注入头追加在头区末尾 |
| 不注入凭据的通道 | **本地**监听不收任何凭据（收到 `Proxy-Authorization` 直接丢弃，不校验）；SOCKS5 本地不收认证 |
| 日志 | 永不记录 `Proxy-Authorization` 的原文或 base64 结果 |

### 4.6 超时、并发与计数

| 项 | 冻结值 | 说明 |
| --- | --- | --- |
| 客户端首包（HTTP 头区 / SOCKS 问候） | **10 s** | 超时直接关闭（见下） |
| 上游 TCP 连接（含 DNS） | **10 s** | 超时 → `502` / `REP=0x01` |
| 上游协商（CONNECT 响应头 / SOCKS 握手+回复） | **10 s** | 同上 |
| 隧道空闲 | **120 s** | **每个方向各自计时**：某方向 120 s 无字节即关闭该连接（写方向也单独计时）。比「任一方向有字节就重置整体计时」更严——另一方向活跃时，静默方向仍会被切断；这是**有意偏差**（避免半开写侧长期占用并发配额），实现见 `pipe::pump` |
| 半关闭 | 一侧 EOF → 对侧 `shutdown(WR)`；另一侧 EOF 后关闭；受空闲超时约束 | 不做 `SO_LINGER` 强杀 |
| 全局并发 | `max_connections`（默认 **256**） | 超限 → HTTP `503` + close / SOCKS `REP=0x01` + close |
| 单 uid 并发 | `max_connections_per_uid`（默认 **64**，`0` 表示不限） | 同上；uid 归属见下 |
| `activeConnections` | 本 listener 当前连接数（含握手中） | 出现在 `relay.listeners[]` |

**uid 归属（单 uid 限流的实现口径，冻结）**：TCP 回环连接无法用 `SO_PEERCRED`（那是 Unix socket 的），因此用内核视图：

1. 取 accepted socket 的对端端口 `peer_port` 与本端端口 `local_port`；
2. 读 `/proc/net/tcp`（IPv4）与 `/proc/net/tcp6`（IPv6 回环），找 `local_address = 127.0.0.1:peer_port` 且 `rem_address = 127.0.0.1:local_port` 的那一行；
3. 取该行的 **`uid` 列**（第 8 列，内核记录的 socket 属主 uid）即客户端 uid；
4. 3 次重试（每次 50 ms）仍找不到 → 按 listener 的 uid 策略处理（**uid 失败口径唯一 = fail-closed，分两个阶段执行**，memo R5）：
   - **`uidPolicy="restricted"`（应用级端口）**：按 §4.6.1 两阶段策略——`relay.firewall.applied=true`（同一次 `-S` 采样确认三链生效）时**放行**并记限频 warning（`reason=uid_unresolved_firewall_enforced`，≤1 次/秒）；`applied=false` 或 `[relay].firewall=false` 时**立即 close**，不读协议字节，日志 `relay client rejected proxy=<name> port=<p> uid=unknown reason=uid_unresolved`。规则被手工 `-F` 后，§4.6.1 的限频复核会在 ≤1s 内把放行切回 fail-closed。
   - **`uidPolicy="all"`（设备级端口，仅 `type="http"` 上游；socks5 默认不建 device listener，见 §3.1）**：接受，只跳过 per-uid 计数（仍受全局 256 约束）。**设备级端口不承诺 per-uid 限额**（memo R11），这不是缺陷。
5. **accept 期 uid 双检**（仅 restricted 端口，顺序固定，**先检后读**）：读出 uid → 若 `uid` 不在 `allowedUids` 中（含 0，集合见 §5.3）→ **立即 close**，日志 `relay client rejected proxy=<name> port=<p> uid=<uid> reason=uid_not_allowed`。**这个判定必须在读取任何协议字节之前完成**（读首字节、SOCKS 问候、HTTP 头区都算「读」）；通过后才进入协议解析。验收用「发 SOCKS 问候收不到 `05 00`」作为「先检后读」的证据（§5.7 第 2 组，R15）。
   - 这一条与 iptables 规则**互为备份**：规则管正常路径，accept 期检查管「规则还没装好 / 正在重建 / 被外部清掉」的例外。
   - 归属失败**不**产生 `egress_relay_uid_unknown`（那个码只用于规则期 `pkg@uid` 解析失败，§5.3）。

限流日志限速：同类事件 10 s 最多一行（`relay limit global` / `relay limit uid=<uid>`）。

> 首包超时**不返回** `408`：此时很可能已经在解析攻击或半开扫描，回包只会给探测者更多信息；直接 `close` 也更简单。

### 4.6.1 uid 归属的两阶段策略（t10 评审补充）

规则期与 accept 期是两套口径，必须分开写清楚：

1. **规则期（重建时）**：pkg@uid 解析失败 → 该端口**不监听**，报 egress_relay_uid_unknown（fail-closed，不变）。
2. **accept 期（每条连接）**：
   - uid 已归属且命中 allowedUids → 放行；
   - uid 已归属但不在 allowedUids → 立即关闭（uid_not_allowed），不变；
   - uid **无法归属**且 relay.firewall.applied=true（同一次 -S 采样确认三链与规则都在）→ 记**限频** warning（≤1 次/秒，reason=uid_unresolved_firewall_enforced）后**放行**；依据是内核 -m owner 已经在 ALLOW 链上按 uid 过滤，accept 双重校验只是第二道防线；
   - uid 无法归属且 firewall.applied=false，或 [relay].firewall=false → **fail-closed 关闭**（uid_unresolved）。

**放行前提（须写进文档与 UI 提示）**：该放行仅在内核规则经 -S 确认生效期间成立。旗标不是永久信任的：accept 期发现 uid 未知时会**限频复核**（≤1 次/秒，单次 -S 带 1 s 超时，见 Revalidator），一旦复核失败立刻回到 fail-closed。规则被手工 -F、被第三方脚本改动、或 iptables 不可用时，放行会在**下一个复核窗口**内失效。

**root-trust 残留风险**：中继以 root 出网，按 uid 生效的第三方防火墙（AFWall+ / NetGuard / 厂商联网管控）看到的都是 root 的连接（§8.1 P8）；本节 uid 过滤只约束「谁能连本地端口」，不约束「连上之后以谁的 uid 出网」。

**pause 口径**：只暂停**本轮需要重建的** listener；未变 listener 可不暂停——pause 不是安全机制，安全由 DENY 链与 accept 期 uid 校验保证（§5.4 论证 5）。

**prune 等价性**：释放端口的 DENY 规则删除可以发生在 accept 恢复之后（两者对已释放端口等价：没有 listener 就没有新连接，残留 DENY 只会多拒一个不再使用的端口）。

### 4.7 失败映射表（数据面）

| 事件 | 本地 HTTP 侧 | 本地 SOCKS5 侧 | 运行态 warning |
| --- | --- | --- | --- |
| 头区超过 32 KiB / 畸形请求 / 非 HTTP/1.1 / 缺 `Host` / `Host` 不符 | `400` + close | — | 无（日志） |
| origin-form 或被拒 scheme | `400` + close | — | 无 |
| 客户端首包超时 | 直接 close | 直接 close | 无 |
| 未授权 uid（已归属但不在白名单） | 直接 close（不读协议） | 直接 close | 无（日志 `uid_not_allowed`） |
| 归属失败（restricted 端口） | `applied=true` → 放行 + 限频日志；`applied=false` / `firewall=false` → 直接 close | 同 | `uid_unresolved_firewall_enforced`（限频日志，不是 warning 码） |
| 上游 TCP / DNS 失败 | `502` | `REP=0x01` | `egress_relay_upstream_unreachable`（`reason=connect`/`dns`） |
| 上游握手超时 | `502` | `REP=0x01` | 同上（`reason=timeout`） |
| 上游 HTTP `407` | `407` **原样回传** + close | — | `reason=auth_failed` |
| 上游 SOCKS RFC1929 失败 | — | `REP=0x01` + close | `reason=auth_failed` |
| 上游 `REP != 0x00` | — | 原样 `REP` + close | 无（目标侧问题） |
| 上游响应头超过 32 KiB / 非 HTTP 响应 | `502` | `REP=0x01` | `reason=protocol_error` |
| 全局 / 单 uid 超限 | `503` + close | `REP=0x01` + close | 无（日志） |
| `CMD != CONNECT` | — | `REP=0x07` | 无 |
| `ATYP` 不支持 | — | `REP=0x08` | 无 |
| 方法列表无 `0x00` | — | `05 FF` + close | 无 |
| relay 监听未就绪（端口未监听） | 内核回 `ECONNREFUSED`（不是中继回包） | 同 | `egress_relay_bind_failed`（§7.1） |

- **应用侧收到的 502 / 407 / REP 都是数据面结果，不是 API 错误码**：不新增对公错误码（egress-contract §6.2「错误码不变」口径保持）。
- `502` 语义与既有 `egress.probe_failed`（探测 502）**不同层**：一个来自数据面、一个来自 API 探测，客户端不应混淆。

### 4.8 日志与凭据边界（增量部分）

沿用 egress-contract §7.1，并补充 relay 专属口径：

- 允许记录：代理名、`local_port`、scope、uid、`reason`、`exitCode`、字节数（可选）。
- 禁止记录：`username`、`password`、`Proxy-Authorization`（原文与 base64）、上游返回的 `Proxy-Authenticate` 值、客户端请求行里的用户信息（`user@host`）。
- 允许的日志样例（每条一行）：

      relay listen proxy=hk_resi port=17800 scope=device uid_policy=all
      relay listen proxy=jp_socks port=17831 scope=app uid_policy=restricted uids=2
      relay upstream connect failed proxy=hk_resi reason=connect
      relay upstream auth failed proxy=jp_socks reason=auth_failed
      relay client rejected proxy=jp_socks port=17831 uid=2000 reason=uid_not_allowed
      relay client rejected proxy=jp_socks port=17831 uid=unknown reason=uid_unresolved
      relay limit uid=10123 active=64 max=64
      relay port reallocated proxy=jp_socks from=17831 to=17835 reason=addr_in_use
      relay listener exited proxy=hk_resi port=17800 error=accept_failed
      relay firewall apply failed op=apply reason=owner_match_unsupported
      relay config degraded key=base_port value=20000 (using 17800)

- 审计（`api-audit.jsonl`）**不新增**请求体快照；`POST /egress/relay/restart` 只记路径与状态码（与既有写端点一致）。

---

## §5 uid 防火墙（`DF_EGRESS_RLY` 链族，fail-closed）

### 5.1 三条链、挂接与绑定面硬断言

**结构（v1.1 起，memo R1）**：单链「清空 → 逐条加」做不到零空窗，改成「入口链 + 可重建 ALLOW + 静态 DENY」：

| 链 | 作用 | 重建策略 |
| --- | --- | --- |
| `DF_EGRESS_RLY` | **入口/分派**（链名 K2 冻结）：`-j ALLOW` → `-j DENY` → `-j RETURN` | 只在缺链/缺跳转时创建；常规重建**不改**它 |
| `DF_EGRESS_RLY_ALLOW` | 只有 ACCEPT（root / 设备级端口全 uid / 应用级端口按 uid） | **全量重建**：`-F` 后按序 `-A` |
| `DF_EGRESS_RLY_DENY` | 每个中继端口一条 REJECT | **只按 spec 增删，永不 `-F`** |

    # 幂等：三条链不存在则创建；OUTPUT 跳转不存在则插到第 1 条
    iptables -w 2 -N DF_EGRESS_RLY        2>/dev/null || true
    iptables -w 2 -N DF_EGRESS_RLY_ALLOW  2>/dev/null || true
    iptables -w 2 -N DF_EGRESS_RLY_DENY   2>/dev/null || true
    iptables -w 2 -C OUTPUT -j DF_EGRESS_RLY 2>/dev/null || iptables -w 2 -I OUTPUT 1 -j DF_EGRESS_RLY

    # 入口链的固定三条（-C 幂等）
    iptables -w 2 -C DF_EGRESS_RLY -j DF_EGRESS_RLY_ALLOW 2>/dev/null || iptables -w 2 -A DF_EGRESS_RLY -j DF_EGRESS_RLY_ALLOW
    iptables -w 2 -C DF_EGRESS_RLY -j DF_EGRESS_RLY_DENY  2>/dev/null || iptables -w 2 -A DF_EGRESS_RLY -j DF_EGRESS_RLY_DENY
    iptables -w 2 -C DF_EGRESS_RLY -j RETURN              2>/dev/null || iptables -w 2 -A DF_EGRESS_RLY -j RETURN

**绑定面硬断言（memo R4，不是散文）**：

1. bind 目标**只能是 IPv4 字面量 `127.0.0.1:<port>`**：禁止 `0.0.0.0`（会变成 LAN 开放代理）、`::`（双栈，绕开 IPv4 规则）、`[::1]`、主机名 `localhost`（解析顺序不可控）。
2. `bind` + `listen` 成功后**必须** `getsockname()` 复核：`family == AF_INET` **且** `addr == 127.0.0.1` **且** `port == local_port`。任一不符 → **关闭 socket、不监听**，warning `egress_relay_bind_failed`（`reason="not_loopback"`）。
3. `SO_REUSEADDR` 只为「同端口重建」；**不用** `SO_REUSEPORT`（它允许两个 socket 同时监听同一端口，破坏「一个定义一个 listener」）。
4. 只做 **IPv4**（`iptables`）：**不安装 `ip6tables` 规则**。这条取舍的合理性由第 1–2 条保证（只 bind IPv4 回环、注入值固定 `127.0.0.1`），验收见 §5.7 第 1 组。
5. 规则只匹配 **17800–17899** 里**实际在端口表上**的端口，不碰其它端口、不碰 INPUT/FORWARD。

### 5.2 规则内容与顺序（具体命令，顺序即语义）

设：设备级端口 `P_dev = base_port`；应用级端口及其允许 uid 集合为 `{(P_app, [u1, u2, ...]), ...}`；`P_all = {P_dev} 并上 {全部 P_app}`；`P_deny = 端口表里全部 local_port`（含本轮解析失败 / 未监听的端口；`P_all` 是它的子集）。

    # ── A. DENY（静态兜底）：先保证「每个端口都有 REJECT」 ───────────────────
    # 对 P_deny 里每个尚未出现的端口（-C 幂等）：
    iptables -w 2 -C DF_EGRESS_RLY_DENY -p tcp --dport 17800 -j REJECT --reject-with icmp-port-unreachable 2>/dev/null || iptables -w 2 -A DF_EGRESS_RLY_DENY -p tcp --dport 17800 -j REJECT --reject-with icmp-port-unreachable
    # ... 每个 P_deny 端口一条

    # ── B. ALLOW（可重建）：先整链清空，再按序追加 ───────────────────────────
    iptables -w 2 -F DF_EGRESS_RLY_ALLOW

    # B1) root（uid 0）永远放行：排障/自测用；也是显式事实
    iptables -w 2 -A DF_EGRESS_RLY_ALLOW -o lo -d 127.0.0.0/8 -p tcp --dport 17800 -m owner --uid-owner 0 -j ACCEPT
    iptables -w 2 -A DF_EGRESS_RLY_ALLOW -o lo -d 127.0.0.0/8 -p tcp --dport 17831 -m owner --uid-owner 0 -j ACCEPT
    # ... 每个 P_all 端口一条

    # B2) 设备级端口：放行【全部 uid】，但只允许回环目的地址
    iptables -w 2 -A DF_EGRESS_RLY_ALLOW -o lo -d 127.0.0.0/8 -p tcp --dport 17800 -j ACCEPT

    # B3) 应用级端口：按解析出的 uid 逐条放行（uid 升序，端口升序）
    iptables -w 2 -A DF_EGRESS_RLY_ALLOW -o lo -d 127.0.0.0/8 -p tcp --dport 17831 -m owner --uid-owner 10123 -j ACCEPT
    iptables -w 2 -A DF_EGRESS_RLY_ALLOW -o lo -d 127.0.0.0/8 -p tcp --dport 17831 -m owner --uid-owner 10200 -j ACCEPT

    # ── C. 收尾：已释放端口从 DENY 删除（放在 accept 恢复之后，见 §5.4 第 10 步）──
    # iptables -w 2 -D DF_EGRESS_RLY_DENY -p tcp --dport 17835 -j REJECT --reject-with icmp-port-unreachable

顺序与理由（不可变）：

1. **DENY 先于 ALLOW 变化**：新端口必须先有 REJECT，才允许出现 ACCEPT。反过来会出现「ACCEPT 已生效、DENY 还没有」的瞬间；虽然那时 listener 还没 accept（§5.4），但规则层必须自身安全，不能依赖调用顺序。
2. **ALLOW 只有 ACCEPT、DENY 只有 REJECT**：重建 ALLOW 时「清空 → 逐条加」，空链期间所有中继端口都落到 DENY → **全拒**（fail-closed），而不是「无规则 → 默认放行」。
3. **ALLOW 内部顺序**：root → 设备级（全 uid）→ 应用级（逐 uid）。root 在最前让排障/验收不被自己的规则挡住；设备级在应用级前是因为它不参与 uid 解析（§3.1）。
4. **DENY 永不 `-F`**：它没有「中间态」。增端口用 `-C` + `-A`；删端口用精确 `-D`，且放在 accept 恢复之后（避免把仍在监听的端口提前打开）。
5. **拒绝用 `--reject-with icmp-port-unreachable`**：应用侧看到 `ECONNREFUSED`（与「端口没人监听」同症状，不给探测者区分信息）。
6. **未命中即 RETURN**：入口链第 3 条 `RETURN` 让非中继端口照旧走 OUTPUT 的其它规则，链只做增量。

「设备级放行全部 uid 但拒绝非回环源」：B2 用 `-o lo -d 127.0.0.0/8` 限定；DENY 对**同一端口**上任何非回环目的地址一律 REJECT。代价（明确接受）：无法访问公网上恰好用 17800–17899 端口的服务；需要时可把 `[relay].firewall` 设为 `false`（不推荐，见 §5.5）。

### 5.3 应用级端口的 `pkg@uid` 解析（规则期）

- 允许 uid 集合 `allowedUids` = {0} 并上 { 解析出的 uid ｜ 包属于引用该定义的 `[[apps]].egress` }。
- 只对**应用级端口**解析；设备级端口不解析（`uid_policy=all`）。
- 解析命令（按序尝试，命中即止；**单次解析 5 s 上限**，超时按失败处理）：

      # 1) 首选：带 uid 的包列表（Android 8+）
      pm list packages -U --user <userId> <package>
      # 输出形如：package:com.example.a uid:10123
      # 2) 后备
      cmd package list packages -U --user <userId> <package>
      # 3) 用户枚举（包名未带 @userId 时）
      cmd user list          # 或 pm list users

- 包名带 `@<userId>` → 只解析该 `userId`；否则对每个已存在用户解析一次，命中的 uid 全部加入。
- 去重（共享 uid 只留一个）、升序后生成规则；同一份 `allowedUids` 也用于 accept 期 uid 双检（§4.6 第 5 条）。
- **规则期解析失败 = 端口不监听**（fail-closed，唯一口径）：包未安装 / `userId` 不存在 / `pm` 不可用（`Unavailable`、非零退出、超时）/ 输出无法解析 → 该端口 `listening=false`、warning `egress_relay_uid_unknown`、**该端口不进 ALLOW**（但仍在 DENY 里，因此也无法被连接）。端口表保留、不重分配（避免抖动）。

### 5.4 listener 生命周期与 fail-closed 重建（memo R1/R2/R6/R7）

**listener 生命周期（冻结，五步 + accept 后双检；任何一步失败都不 accept）**：

| 步 | 动作 | 失败后果 |
| --- | --- | --- |
| 1 | **create(paused)**：`bind(127.0.0.1:port)` + `listen(backlog)`，**不 accept** | 该端口本轮不监听（`egress_relay_bind_failed`） |
| 2 | **`getsockname` 断言**：`AF_INET` + `127.0.0.1` + 端口相符（§5.1） | 关闭 socket、不监听，`reason="not_loopback"` |
| 3 | **解析 `pkg@uid`**（仅 app 端口，5 s 上限，§5.3） | 关闭 socket、不监听，`egress_relay_uid_unknown` |
| 4 | **装规则**（DENY 已含该端口 + ALLOW 全量重建，§5.2） | 关闭 socket、不监听，`egress_relay_firewall_failed` |
| 5 | **校验 `-S`**：入口链三条跳转、ALLOW 逐条、DENY 含该端口，全部与期望一致 | 同上（不一致 = 校验失败，**不 accept**） |
| 6 | 通过后才 **accept**；accept 后立即做 uid 双检（§4.6 第 5 条） | — |

**重建算法（冻结；启动 reconcile / `PUT /egress` / `DELETE /egress` / `POST /egress/relay/restart` 共用同一条流水线）**：

    1. 计算期望态（端口表 / 每端口 uid 集合 / firewall 开关）。**短路条件（v1.2 收紧，R14）**：期望态 == 内存态 **且** `iptables -S` 三链与期望逐条一致 **且** 本次不是 `POST /egress/relay/restart`；三者同时成立才跳到 9（不动 listener、不动规则、不写文件）。显式 restart **永不短路**——否则手工 `-F` 后 restart 不修复，`firewall.applied` 还会说谎。
    2. 暂停**本轮需要重建的** listener 的 accept（含即将被关闭的；不关 socket；已建立的连接不受影响）。**未变 listener 可以不暂停**——pause 不是安全机制，安全由 DENY 链与 accept 期 uid 校验保证（§4.6.1「pause 口径」）。
    3. 解析 app 端口的 pkg@uid（5 s 上限；失败者本轮不监听）
    4. 关闭「端口变化 / uid 集合变化 / 需重建」的旧 listener —— 先关后开，防 EADDRINUSE
    5. 端口分配：按 §1.3 直接 bind 成 paused socket（本进程刚释放的端口不算外部占用）
    6. 端口表有变化 → 原子写回 egress.toml（0600）；写失败 → 相关新 listener 不 accept
    7. 链与跳转幂等存在；DENY 增补 P_deny 里缺失的端口（-C + -A）
    8. -F DF_EGRESS_RLY_ALLOW，按 §5.2 B1→B3 顺序 -A；随后 -S 三链校验
    9. 恢复 accept（仅通过 1–8 的 listener）
    10. 删除已释放端口的 DENY spec；刷新内存状态（listening / portChanges / restartRequiredApps）。实现上 prune 可能发生在本轮 accept 恢复之前——与被关闭端口**等价**（没有 listener 就没有新连接，残留 DENY 只会多拒一个不再使用的端口，§4.6.1「prune 等价性」）。

**「重建期间无裸奔」的论证（评审用，逐条可验）**：

1. 任何时刻 `DF_EGRESS_RLY_DENY` 都包含 `P_deny` 里**全部端口**的 REJECT：「只增不减」，删除只发生在 accept 恢复之后（第 10 步）。因此不存在「某端口没有兜底 REJECT」的时间点。
2. 重建只清空 `ALLOW`；空 `ALLOW` 不产生任何 ACCEPT，目标端口**必然**落到 `DENY` 的 REJECT。`-F` 与第一条 `-A` 之间的毫秒级间隙因此是**全拒**，不是放行。
3. 新增端口时 DENY 先加（第 7 步）、listener 最后才 accept（第 9 步）——不会出现「端口已监听、DENY 里没有它」。
4. 已建立的连接不受规则变更影响（内核语义）；它们在上一次 accept 时已过 uid 双检（§4.6），因此无需重判。重建期间**新**连接即使被内核排进 backlog，恢复 accept 后仍要过一次 uid 双检，未授权 uid 拿不到隧道。
5. 所以「暂停 accept」不再是安全机制（它挡不住内核完成 TCP 握手，memo R1 已指出），**只是可用性优化**；安全由「DENY 兜底 + accept 期 uid 双检」两层保证。

- **首次启动的时序**：三条链与 OUTPUT 跳转在**任何 listener accept 之前**装好；listener 从 paused 开始，因此不存在「先服务后建规则」的窗口。
- **幂等**：同一份期望态重复应用 → 若 `-S` 三链已一致则短路（`rules` 计数不变、不产生重复跳转）；`POST /egress/relay/restart` 例外：**强制 apply**，但重装后 `-S` 输出与上次逐条相同——这才是「幂等」的正确含义（R14）。
- **`firewall.applied` 由 `-S` 推出**：`GET /egress` 每次现算三链 `-S` 并与期望比对；写响应复用本次重建的校验结果。手工 `-F` 之后，下一次 GET 必须报 `applied=false`，restart 必须把它修回 `true`。
- `iptables -w` 兼容：`unknown option "-w"` → 去掉 `-w 2` 重试一次并记 warn；两次都失败按 §5.5。
- **`relay.firewall` 计数口径**：`rules` = 三条链的规则总条数（入口链 3 + ALLOW + DENY，不含 OUTPUT 跳转）；另给 `allowRules` / `denyRules` 便于验收断言（§6.1）。

### 5.4.1 端口释放窗口与本地端口劫持（t10 评审补充）

关闭旧 listener（reconcile / restart / DELETE）后，`127.0.0.1:178xx` 会短暂处于「无人监听」；**任何本机进程**（无需 root，端口 ≥1024）都可能在这个窗口里抢占该端口。应用仍指向旧 `local_port` 时，连接会落到**抢占者**而不是中继。

- **服务只信任自己 bind 成功的 socket**：所有转发都发生在自己 hold 的 listener 上；发现端口已被占用的唯一信号是**下一次 bind 失败**（`addr_in_use`）。`-S` 复核只校验 iptables 规则，**不能**用来判断端口属主。
- **检测与失败行为**：
  - 应用级：按 §1.3 线性探测换端口 → 原子写回 `egress.toml` → `relay.portChanges{reason:"addr_in_use"}` → `relay.restartRequiredApps` 列出引用该定义、需要重启的应用；旧端口上的连接在重启前会被拒绝或落到抢占者。
  - 设备级（只有 `base_port` 一个候选）：listener 不监听，`egress_relay_bind_failed{scope:"device", reason:"addr_in_use"}`；`settings` 按 §7.1 **不指向中继**（清成直连），不写一个没人监听的端口。
  - listener 运行期中途退出（`listener_exited`）走同一套：先退避重绑（1s/5s/15s，最多 3 次），仍失败就停在「失败 + 可见」，等 restart。
- **残留风险（写进文档）**：抢占要赢得毫秒级窗口；应用使用旧端口且尚未重启时，无法从服务侧阻止它连到抢占者。缓解：把 `relay.portChanges` 当作**唯一权威信号**并立刻按 `restartRequiredApps` 重启应用；root 排障时用 `ss -ltnp | grep 178` 核对端口属主是否为 `device_faker_server`；不要把 `[relay].firewall=false` 当成排障手段（它同时会削弱 accept 期双检的兜底）。
### 5.5 失败策略

| 情形 | 行为 |
| --- | --- |
| `iptables` 不存在 | `egress_relay_firewall_failed`（`reason="iptables_missing"`）；**本轮需要重建的应用级端口不监听**（未变 listener 保持监听、靠 accept 期双检兜底，见补充 B）；设备级 listener 照常（不依赖防火墙，§0 D2）；`firewall.applied=false` |
| `-m owner` 不支持 | 同上（`owner_match_unsupported`） |
| 命令非零退出 | 同上（`command_failed`，带 `exitCode`） |
| 非 root / 权限拒绝 | 同上（`permission_denied`） |
| `-S` 校验与期望不符 | 同上（`command_failed`），且**不 accept**（宁可不可用，不裸奔） |
| `[relay].firewall=false`（显式关闭） | **不建链、不建规则**（`firewall.applied=false`、`rules=0`、不产生 warning）；应用级端口照常监听，但 **accept 期 uid 双检仍然生效**（§4.6 第 5 条）：未授权 uid 与归属失败一律 close。它**不是**「全 uid 可用」（R16） |
| 清理失败（卸载 / 关闭中继） | `egress_relay_firewall_failed`（`op="clear"`） |

补充 A（v1.2 统一口径，R16）：`firewall=false` 时**规则层不设防**——设备级端口本来就全 uid 可达；应用级端口仍由 accept 期双检按 `allowedUids` 拒绝（归属失败也 close）。因此关掉防火墙只是把「内核先拒」退化成「服务层再拒」，未授权 uid 仍拿不到隧道；凭据暴露面**小于**「全 uid 可用」的说法。它仍是「排障开关」、不能作为默认；`GET /egress.relay.firewall.applied=false` 是显式可见事实。

补充 B（**未变 listener 不受本轮 apply 失败影响**，v1.11.0 实现口径）：apply 失败只让**本轮需要重建**的应用级 listener 不监听（paused socket 直接关闭，`lastError.code=egress_relay_firewall_failed`）；**端口与 uid 集合未变的 listener 保持监听**——它们的规则可能已经被 `-F ALLOW` 清空，此刻唯一防线是 **accept 期 uid 双检**（§4.6 第 5 条）。因此本节的「应用级端口全部不监听」应理解为「**本轮重建的应用级端口**」，不是「所有应用级端口」，也不是「未变 listener 被关闭」。

补充 C（**可用性抖动窗口**）：重建 `DF_EGRESS_RLY_ALLOW` 时（`-F` 到首条 `-A` 之间、以及整个重建期间），**未变 listener 的新连接也会短暂全拒**——因为入口链先走 ALLOW，空 ALLOW 不产生 ACCEPT，随后落到 DENY 的 REJECT。窗口是毫秒级，方向是**安全失败**（拒绝而不是裸奔）；已建立的连接不受影响。这个抖动换来的是「任何时刻都不存在无规则窗口」，属有意取舍。

### 5.6 卸载与清理（幂等，三条链）

    # 服务正常退出（SIGTERM / 退出兜底），或 [relay].enabled=false 时：
    # 1) 摘 OUTPUT 跳转 → 2) 删叶子链 ALLOW → 3) 删叶子链 DENY → 4) 删入口链
    iptables -w 2 -D OUTPUT -j DF_EGRESS_RLY 2>/dev/null || true
    iptables -w 2 -F DF_EGRESS_RLY_ALLOW 2>/dev/null || true
    iptables -w 2 -X DF_EGRESS_RLY_ALLOW 2>/dev/null || true
    iptables -w 2 -F DF_EGRESS_RLY_DENY  2>/dev/null || true
    iptables -w 2 -X DF_EGRESS_RLY_DENY  2>/dev/null || true
    iptables -w 2 -F DF_EGRESS_RLY       2>/dev/null || true
    iptables -w 2 -X DF_EGRESS_RLY       2>/dev/null || true

- **顺序**：先 `-D` 摘掉 OUTPUT 跳转（此后新连接不进链）→ 再按「叶子链 → 入口链」删（被引用的链不能 `-X`）。
- **服务退出只清规则与监听，不清 `settings`**（与既有语义一致：卸载才清系统代理）。
- **`uninstall.sh` 的插入点**：停守护循环 → 停 server → 清三条链（上面六行）→ `settings put global http_proxy :0` + 清排除列表 → `rm -rf /data/adb/device_faker`。前两步顺序不变（否则守护会拉回进程），防火墙清理放在 `settings` 之前（先把门关上再拆代理）。
- **模块被禁用（不跑 `uninstall.sh`）**：链与 `settings` 都可能残留 → 禁用前先 `DELETE /egress`（清 settings）**并**手工清链：

      adb shell su -c 'iptables -D OUTPUT -j DF_EGRESS_RLY; iptables -F DF_EGRESS_RLY_ALLOW; iptables -X DF_EGRESS_RLY_ALLOW; iptables -F DF_EGRESS_RLY_DENY; iptables -X DF_EGRESS_RLY_DENY; iptables -F DF_EGRESS_RLY; iptables -X DF_EGRESS_RLY'
      adb shell su -c 'iptables -S DF_EGRESS_RLY'    # 期望：No chain/target/match by that name

- 规则**不持久化**（不写 `iptables-save`）：每次开机由 server 启动 reconcile 重建。开机到 server 就绪之间端口本来就不监听，不存在「有规则无服务」的窗口。

### 5.7 验收命令（t9 直接用；v1.2 修正版）

> **本机环境约束（t9 实测）**：设备上**没有 `curl`**、**没有 `setpriv`**；`run-as <pkg> toybox nc` 的出网被 SELinux 拦（Timeout）。因此：① 所有 HTTP/API 调用在**主机**执行（`adb forward tcp:8686 tcp:8686`，`$BASE` / `$TOKEN` 见 §10.2），**不要**在设备上找 curl；② **放行证据只能来自「应用内探针」**；③ 设备侧探针统一用 `printf <bytes> | toybox nc -q 3 -w 6 127.0.0.1 <port>`，**`-q` 必带**（否则 nc 拿不到回包就退出）；④ fixture 上游不要用 `127.0.0.1`（`adb reverse` 在本机不稳定，首个请求可达、后续丢包），改用与设备同 /24 的 LAN IP（实测 `192.168.30.149`）。

**第 1 组：结构与绑定面（memo R3/R4）**

    # 三条链都在，入口链是固定的三条跳转
    adb shell su -c 'iptables -S DF_EGRESS_RLY'
    adb shell su -c 'iptables -S DF_EGRESS_RLY_ALLOW'
    adb shell su -c 'iptables -S DF_EGRESS_RLY_DENY'
    adb shell su -c 'iptables -C OUTPUT -j DF_EGRESS_RLY; echo $?'        # 0

    # 只监听 127.0.0.1（IPv4）；IPv6 侧必须为空
    adb shell su -c 'ss -ltn  | grep -E ":178[0-9]{2}"'                   # 每行都是 127.0.0.1:178xx
    adb shell su -c 'ss -ltn6 | grep -E ":178[0-9]{2}"'                   # 必须为空

    # 非 127.0.0.1 目的地连不上（以下三条都不加 su）
    adb shell 'toybox nc -w 2 ::1 17831 </dev/null; echo rc=$?'            # rc 非 0
    adb shell 'toybox nc -w 2 127.0.0.2 17831 </dev/null; echo rc=$?'      # rc 非 0
    adb shell 'toybox nc -w 2 <LAN_IP> 17831 </dev/null; echo rc=$?'       # rc 非 0（<LAN_IP> 用 ip addr show wlan0 取）

**第 2 组：uid 授权（memo R3/R6/R15；`su` 之后是 uid 0，root 恒被放行，不能用来证明「未授权」）**

    # 拒绝用例：**不加 su**（adb shell 是 uid 2000）。判据是协议级——服务端在 accept 后、读首字节前
    # 完成 uid 双检（先检后读），所以连 SOCKS 问候的回复都不会出现；-q 必带
    adb shell 'printf "\005\001\000" | toybox nc -q 3 -w 6 127.0.0.1 17831 | od -An -tx1'   # 期望：空输出（无 05 00）
    adb shell su -c 'grep -c "reason=uid_not_allowed" /data/adb/device_faker/logs/server.log'   # >= 1

    # 允许用例（本机唯一可用）：**应用内探针**——target app（或 fixtures/probe-app）在自身进程里
    # 连 127.0.0.1:<应用级端口>，期望 SOCKS 回 05 00 / HTTP 拿到响应；结果从 logcat 或探针输出读。
    # 注：setpriv 在本机不存在；run-as 出网被 SELinux 拦（Timeout）——这两种写法不要再用。
    adb logcat -d -s ProbeEgress | tail -n 5        # 期望：handshake ok / 200

    # 规则被人为清空：入口链 RETURN -> OUTPUT 默认 ACCEPT -> **内核仍会完成握手**，
    # 因此不能用 nc 的退出码判定（rc 通常 0）；用协议级证据 + 服务端日志
    adb shell su -c 'iptables -F DF_EGRESS_RLY_ALLOW; iptables -F DF_EGRESS_RLY_DENY'
    adb shell 'printf "\005\001\000" | toybox nc -q 3 -w 6 127.0.0.1 17831 | od -An -tx1'   # 期望：空输出（先检后读）
    adb shell su -c 'grep -c "reason=uid_not_allowed" /data/adb/device_faker/logs/server.log'

    # restart 必须强制 apply（R14）；从主机调用（设备无 curl）
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d '{}' \
      "$BASE/egress/relay/restart" | jq '.relay.firewall'      # applied=true、denyRules>0
    adb shell su -c 'iptables -S DF_EGRESS_RLY_DENY' | wc -l   # > 0（规则已恢复）

**第 3 组：重建零空窗（memo R1/R15；判据必须是协议级）**

    # 未授权 uid 持续发 SOCKS 问候（每次独立 adb shell），同时从主机连打 20 次 restart。
    # 判据：「没有一次拿到中继服务（协议级）」——不是 nc 的退出码：重建期监听 socket 仍在 listen，
    # 内核照常完成三次握手，rc 会成功；正确实现是在 accept 后、读首字节前 close（先检后读）。
    ( for i in $(seq 40); do adb shell 'printf "\005\001\000" | toybox nc -q 1 -w 2 127.0.0.1 17831 | od -An -tx1 | grep -q 05 && echo HIT'; done ) &
    for i in $(seq 20); do
      curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d '{}' \
        "$BASE/egress/relay/restart" >/dev/null
    done
    wait                                                       # 输出里必须没有 HIT
    # 交叉证据：期间服务端日志应出现 uid_not_allowed（而不是「连接被接受并服务」）
    adb shell su -c 'grep -c "reason=uid_not_allowed" /data/adb/device_faker/logs/server.log'

**第 4 组：幂等、强制 apply 与端口稳定（memo R6/R14）**

    adb shell su -c 'iptables -S DF_EGRESS_RLY_ALLOW' > /tmp/fw-before
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d '{}' \
      "$BASE/egress/relay/restart" >/dev/null
    adb shell su -c 'iptables -S DF_EGRESS_RLY_ALLOW' > /tmp/fw-after
    diff /tmp/fw-before /tmp/fw-after                     # 无差异（幂等：重装后逐字节相同）
    adb shell su -c 'grep local_port /data/adb/device_faker/config/egress.toml'

    # 强制 apply：手工清空后用 restart 修复，applied 必须由 -S 推出
    adb shell su -c 'iptables -F DF_EGRESS_RLY_ALLOW; iptables -F DF_EGRESS_RLY_DENY'
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress" | jq '.relay.firewall.applied'    # false
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d '{}' \
      "$BASE/egress/relay/restart" | jq '.relay.firewall'   # applied=true、denyRules>0

**第 5 组：fail-closed 端口与卸载清理（memo R12）**

    # 9 端口必须没有监听者（否则 127.0.0.1:9 会连到别的服务，fail-closed 语义被破坏）
    adb shell su -c 'ss -ltn | grep -E ":9( |$)"'          # 期望：空

    # 卸载清理
    adb shell su -c 'sh /data/adb/modules/device_faker/uninstall.sh'
    adb shell su -c 'iptables -S DF_EGRESS_RLY'            # 链不存在
    adb shell su -c 'settings get global http_proxy'       # :0 或 null

---

## §6 API 变更（设备端 `/api/v1`）

通用约定全部沿用 egress-contract §1.1：Bearer Token、作用域、`Cache-Control: no-store`、`X-Request-Id`、统一错误信封；**egress 端点不设 `X-DF-Revision` / `ETag`**（新端点同样不设），egress revision 仍在响应体与 `X-DF-Egress-Revision` 口径里。

### 6.1 `GET /egress` 新增 `relay` 对象

作用域 `read`。响应体在既有字段（egress-contract §1.3）之外**追加**一个顶层 `relay`：

    "relay": {
      "enabled": true,
      "basePort": 17800,
      "failOpen": true,
      "at": "2026-11-25T10:00:00Z",
      "limits": { "maxConnections": 256, "maxConnectionsPerUid": 64 },
      "listeners": [
        {
          "proxy": "hk_resi",
          "port": 17800,
          "scope": "device",
          "listening": true,
          "uidPolicy": "all",
          "allowedUids": [],
          "activeConnections": 3,
          "lastError": null
        },
        {
          "proxy": "jp_socks",
          "port": 17831,
          "scope": "app",
          "listening": true,
          "uidPolicy": "restricted",
          "allowedUids": [10123, 10200],
          "activeConnections": 0,
          "lastError": null
        }
      ],
      "firewall": { "applied": true, "rules": 14, "allowRules": 9, "denyRules": 2, "lastError": null },
      "portChanges": [],
      "restartRequiredApps": []
    }

字段表：

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `relay.enabled` | boolean | 否 | **实际生效值**（`[relay]` 降级后的结果，§2.5） |
| `relay.basePort` | integer | 否 | 实际生效的 `base_port`（17800–17899） |
| `relay.failOpen` | boolean | 否 | `[probe].fail_open` 的**实际生效值**。中继生效且该值为 `true` 时，UI 必须提示「上游探测失败会清系统代理 → 静默直连」（§8.1 P9、§8.3） |
| `relay.at` | string | 否 | 本快照时刻（RFC 3339 UTC） |
| `relay.limits.maxConnections` | integer | 否 | 实际生效的全局并发上限 |
| `relay.limits.maxConnectionsPerUid` | integer | 否 | 实际生效的单 uid 上限（`0` 表示不限） |
| `relay.listeners` | object[] | 否 | **每个「应该监听」的中继端口一条**：带凭据、已有 `local_port`、且被 `[egress].default` 或某个 `[[apps]].egress` 引用；设备级在前，其余按 `port` 升序；没有应监听端口时 `[]`。`listening=false` 表示期望监听但失败（原因见 `lastError`）。端口表有值但**无人引用**的定义不出现（§6.2） |
| `listeners[].proxy` | string | 否 | 代理名（定义表名） |
| `listeners[].port` | integer | 否 | 实际监听端口（等于 `local_port`） |
| `listeners[].scope` | `"device"` 或 `"app"` | 否 | `device` = 设备级端口（`port == basePort`） |
| `listeners[].listening` | boolean | 否 | 是否正在 accept；`false` 时必有 `lastError` |
| `listeners[].uidPolicy` | `"all"` 或 `"restricted"` | 否 | `device` 恒为 `all`；`app` 恒为 `restricted` |
| `listeners[].allowedUids` | integer[] | 否 | `restricted` 时为解析出的 uid（升序，含 0）；`all` 时为 `[]`（不展开全 uid） |
| `listeners[].activeConnections` | integer | 否 | 当前连接数（含握手中；进程重启清零） |
| `listeners[].lastError` | object 或 null | 是 | `{code, reason, at}`；`code` 属于 §6.4 的 relay 警告码，`reason` 为该码 `details.reason` 的值，`at` 为最近一次失败时刻 |
| `relay.firewall.applied` | boolean | 否 | **由 `iptables -S` 推出**：`GET /egress` 每次现算三链并与期望比对（R14）；`firewall=false` 或校验失败时为 `false`，手工 `-F` 后下一次 GET 必须变 `false` |
| `relay.firewall.rules` | integer | 否 | 三条链规则总条数（入口链 3 + ALLOW + DENY，不含 `OUTPUT` 跳转） |
| `relay.firewall.allowRules` | integer | 否 | `DF_EGRESS_RLY_ALLOW` 的条数（便于验收断言「链内无该端口 ACCEPT」） |
| `relay.firewall.denyRules` | integer | 否 | `DF_EGRESS_RLY_DENY` 的条数（必须等于 `P_deny` 的端口数，§5.4） |
| `relay.firewall.lastError` | object 或 null | 是 | `{code:"egress_relay_firewall_failed", reason, op, at}` |
| `relay.portChanges` | object[] | 否 | **仅写响应可能非空**；GET 恒 `[]`。元素 `{proxy, from, to, reason}`（`from` 为 `null` 表示首次分配；`reason` 属于 `initial` / `collision` / `out_of_range` / `addr_in_use` / `role_change` / `credentials_change`） |
| `relay.restartRequiredApps` | string[] | 否 | **仅写响应可能非空**；GET 恒 `[]`。§1.7 的包名列表（升序去重） |

补充规则：

- `relay` **恒存在**（即使 `enabled=false`、即使一个 listener 都没有）。
- `upstream` / `proxies[]` 的 `host`/`port` 永远是**上游真实地址**；中继地址只在 `relay.listeners[].port` 与 `proxies[].localPort`。
- `health` / `readback` / `applied` 语义不变（中继生效时 `readback.settingsValue` 会是 `127.0.0.1:17800`）。
- PUT / DELETE 响应也同样带 `relay`（与 GET 同构），因此写响应里能看到 `portChanges` / `restartRequiredApps`。
- **不新增对公错误码**。

### 6.2 `GET /egress/proxies` 新增两个字段

`items[]` 追加（与 `GET /egress` 的 `proxies[]` **同构**，两处必须一致）：

| 字段 | 类型 | 可空 | 说明 |
| --- | --- | --- | --- |
| `localPort` | integer 或 null | 是 | 端口表值；无分配（无凭据 / relay 关闭 / 池耗尽）时为 `null` |
| `relayActive` | boolean | 否 | 该定义当前是否**正在经中继**：`[relay].enabled` 且 `hasCredentials` 且 `localPort` 非 null 且对应 listener 正在监听 |

- `relayActive` 是**运行时**事实（进程重启后按重建结果变化），`localPort` 是**持久化**事实。
- 未建立 listener 但端口表有值的定义（未被任何 `[[apps]].egress` 与 `[egress].default` 引用）→ `localPort` 有值、`relayActive=false`（§1.2：端口表不等于监听）。
- 响应其余部分（`total`、`usedAsDefault`、`apps`、`revision`、`configRevision`）不变。

### 6.3 新增 `POST /api/v1/egress/relay/restart`

- 作用域 **`ops`**；无 `If-Match`；不支持 `Idempotency-Key`（与 `POST /egress/probe` 同口径：它是动作而不是配置写入）。
- 用途：重新读取 `[relay]` 与 `[proxies]`，**全量重建** listener 与防火墙链，并重放设备级 `settings`（不清配置）。**跳过 §5.4 的短路**（强制 apply，R14）：即使内存态与期望一致、`-S` 也一致，也要重装一遍规则与监听。
- 请求体：可为空对象 `{}`；**不接受任何字段**（出现字段 → `422 unknown_field`）。
- 响应 `200`：

      {
        "restartedAt": "2026-11-25T10:05:00Z",
        "relay": { ...与 §6.1 同构... },
        "warnings": []
      }

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `restartedAt` | string | 本次重建完成时刻（RFC 3339 UTC） |
| `relay` | object | 重建后的完整 relay 状态（§6.1 字段表，含 `portChanges` / `restartRequiredApps`） |
| `warnings` | object[] | §6.4 |

- **幂等（v1.2 口径，R14）**：重复调用不写 `config.toml`、不重新分配端口（除非 §1.5 C2/C3）；但**每次都强制重装规则与监听**（不短路），以保证手工 `-F` 后能被 restart 修复；端口无变化时 `portChanges:[]`、`restartRequiredApps:[]`，且重装后 `-S` 与上次逐条一致。
- 副作用清单（写清楚，别让调用方意外）：
  1. **所有 listener 暂停 accept**（毫秒级；期间中继端口由 `DF_EGRESS_RLY_DENY` 全拒，不裸奔）；端口与 uid 集合**未变**的 listener 不关闭、不中断连接；
  2. 端口/uid 集合变化的 listener 才关闭重建（**先关后开**，§5.4）；
  3. 重建 `DF_EGRESS_RLY_ALLOW`（`-F` + 按序 `-A`）并按 §5.4 校验；`DF_EGRESS_RLY_DENY` 只增删 spec。**restart 无条件执行本步**（即使 `-S` 已一致），这是「强制 apply」的落点；
  4. 若设备级中继生效，重放 `settings put global http_proxy 127.0.0.1:<base_port>`（读回校验照旧）；
  5. 可能写一次 `egress.toml`（仅当发生端口重分配/补齐），从而推进 egress revision。
- 失败：
  - `egress.toml` 语法错误 → `500 internal_error`（`details.op="egress_parse"`）。
  - 设备级 `settings` 不可用（`enabled=true` 且设备级走中继时）→ `503 upstream_unavailable`（`details.component="settings"`）。listener/防火墙照常重建，客户端可再 `GET /egress` 看状态。
  - `[relay].enabled=false` → `200`，`relay.enabled=false`、`listeners:[]`，并卸载链（不是错误）。
- 日志：`relay restarted listeners=<n> firewall_applied=<bool>`（一行，不含配置内容）。

### 6.4 新增 5 个警告码（扁平，`Warning.code`）

`Warning` 形状沿用 `{code, message, details}`（[../API.md](../API.md) §6.6）。**只新增这 5 个**；`details` 形状与 `reason` 枚举冻结如下（`reason` 只增不改）。

| code | details | 触发（状态反射，不是事件流） |
| --- | --- | --- |
| `egress_relay_bind_failed` | `{proxy, port, scope, reason}`：`proxy` string；`port` integer 或 null；`scope` 属于 `"device"` / `"app"`；`reason` 属于 `"addr_in_use"` / `"permission_denied"` / `"address_not_available"` / `"not_loopback"` / `"pool_exhausted"` / `"persist_failed"` / `"listener_exited"` / `"other"` | 该中继端口绑定/持久化失败、或 listener 中途退出。`port` 在 `pool_exhausted` 时为 `null` |
| `egress_relay_firewall_failed` | `{op, reason, chain, ports, exitCode}`：`op` 属于 `"apply"` / `"clear"`；`reason` 属于 `"iptables_missing"` / `"owner_match_unsupported"` / `"command_failed"` / `"permission_denied"`；`chain` 属于 `"DF_EGRESS_RLY"` / `"DF_EGRESS_RLY_ALLOW"` / `"DF_EGRESS_RLY_DENY"`；`ports` 为 integer[]（升序，`op="clear"` 时可为空）；`exitCode` 为 integer 或 null | 防火墙规则未能落地或未能清理 |
| `egress_relay_uid_unknown` | `{proxy, port, scope, package, userId, reason}`：`scope` 恒为 `"app"`；`package` string；`userId` integer 或 null；`reason` 属于 `"package_not_installed"` / `"user_not_found"` / `"pm_unavailable"` / `"parse_failed"` | 应用级端口的 `pkg@uid` 解析失败 → 该端口**不监听**（§5.3） |
| `egress_relay_upstream_unreachable` | `{proxy, port, scope, reason, at}`：`scope` 属于 `"device"` / `"app"`；`reason` 属于 `"connect"` / `"dns"` / `"timeout"` / `"auth_failed"` / `"protocol_error`；`at` 为该次失败时刻 | 数据面最近一次到上游的握手失败；该 listener 下一次握手成功后自动消失 |
| `egress_relay_device_socks5_unsupported` | `{proxy, scope, reason}`：`scope` 恒为 `"device"`；`reason` 恒为 `"socks5_device_scope_unsupported"` | `[egress].default`（合并后的有效值）指向**非 http**（socks5）定义：不建 device listener；`enabled=true` 时 `settings` 写黑洞 `127.0.0.1:9`，`enabled=false` 时按未配置（直连）；`PUT` 对这种 default 直接 `422`。与是否带凭据无关；`enabled=false` 时仍报（配置提醒，不是运行态故障） |

出现位置与生命周期：

| 项 | 口径 |
| --- | --- |
| 出现位置 | `GET /egress`、`PUT /egress`、`DELETE /egress`、`POST /egress/relay/restart` 的 `warnings` |
| 不出现在 | `POST /egress/probe`（§3.5）、数据面响应（§4.7） |
| 去重 | 每个 listener 最多一条 relay 警告（`lastError` 与 `warnings` 同源）；`firewall` 最多一条 |
| 清除 | 对应问题消失（listener 重新监听成功 / 规则落地 / 上游握手成功）→ 下次响应不再出现 |
| 静默条件 | `[relay].enabled=false` → 不产生 relay **运行态**警告（bind / firewall / uid / upstream；关闭状态不是故障）；**例外**：`egress_relay_device_socks5_unsupported` 是配置提醒，只要合并后的有效 `default` 指向非 http 定义就报，与 `enabled` 无关；`firewall=false` → 不产生 `egress_relay_firewall_failed`（显式配置） |
| 不新增第 6 个码 | 端口重分配**不是故障**，用 `relay.portChanges` + `relay.restartRequiredApps` 表达（§1.7）；`[relay]` 段的非法值用降级 + 实际值表达（§2.5）；设备级 socks5 配置用 `egress_relay_device_socks5_unsupported` 表达（§3.1） |

### 6.5 `PUT /egress`：`local_port` 只读（`422 unknown_field`）

- 请求体 `proxies[]` 项出现 `localPort`（camelCase，线上名）或 `local_port`（snake_case，TOML 名）→ **`422 validation_failed`**：

      {
        "error": {
          "code": "validation_failed",
          "message": "请求体字段不合法",
          "details": {
            "errors": [
              { "path": "proxies[1].localPort", "code": "unknown_field",
                "message": "localPort 由服务端分配，客户端只读" }
            ]
          },
          "retryable": false,
          "requestId": "req_..."
        }
      }

- 两种写法都要拒：`localPort` 是线上命名，说明客户端试图写只读字段；`local_port` 是 TOML 命名，说明它从文件里抄过来的——两者都属未知字段，行为一致（`path` 保留客户端原文）。
- 顶层 `relay` / `relayEnabled` / `relayBasePort` / `relayFirewall` → 同样 `422 unknown_field`（§2.2：`[relay]` 本轮不由 API 写入）。
- 同一请求里若同时有其它合法变更 → 整体 `422`，**不落盘**（与既有校验一致性相同：校验先于写入）。
- `GET /egress` 与 `GET /egress/proxies` 的 `localPort` 回显不受影响（只读可见）。

### 6.6 端点总表、作用域与计数（供 t8 文档收口）

| # | 方法 | 路径 | scope | 变化 |
| --- | --- | --- | --- | --- |
| 25 | GET | `/api/v1/egress` | `read` | **响应增 `relay`** |
| 26 | PUT | `/api/v1/egress` | `write` | 响应增 `relay`；`localPort`/`local_port`/`relay*` → 422 |
| 27 | DELETE | `/api/v1/egress` | `write` | 响应增 `relay`；`[relay]` 保留 |
| 28 | POST | `/api/v1/egress/probe` | `ops` | **请求 +1 字段 `via`**；**响应 +1 字段 `relayPort`**，`via` 值域 +`"relay"`；`reason` +2（`relay_not_listening` / `relay_connect`，§3.5） |
| 29 | GET | `/api/v1/egress/proxies` | `read` | **增 `localPort` / `relayActive`** |
| 30 | POST | `/api/v1/egress/relay/restart` | `ops` | **新增** |

- 设备端 `/api/v1` 端点数：**29 → 30**；`CONTRACT_OPERATIONS` 常量同步 +1，`CAPABILITIES` 仍为 `egress`（不新增能力名）。
- 文档：`docs/API.md` §4 的端点总表 **+1**（新 §5.30），§5.25–§5.29 按 §9.3 增补。
- 幂等矩阵增补（接 egress-contract §1.9）：

| 端点 | 幂等？ | 说明 |
| --- | --- | --- |
| `POST /egress/relay/restart` | 条件幂等 | 重复调用最终状态一致；端口无变化时不写文件；`restartedAt` 每次变化；进行中的连接被中断（副作用，不是不幂等） |

### 6.7 Manager / WebUI 取用口径（t6 / t7 / t8）

| 位置 | 取什么 | 展示建议 |
| --- | --- | --- |
| `GET /relay/api/v1/devices/{id}/egress` 的 `deviceState` | **原样透传**设备端 `relay`（不裁剪字段，egress-contract §6.8 已有口径） | 详情页显示「设备内中继：已启用 / N 个监听」 |
| `DeviceSummary.egress` | 新增 `relay: {enabled, active}`（t6） | 设备卡上的「经设备中继」徽标；`active = enabled && 至少一个 listener listening` |
| Manager 侧代理池 | 不新增字段（代理池是 Manager 自己的数据） | 代理详情提示「带凭据的代理在设备上经本地中继生效」 |
| 设备端 WebUI「网络出口」卡片 | `relay.listeners` / `firewall` / `relay.failOpen` / `restartRequiredApps` | 每行「代理名 → 127.0.0.1:端口（监听中/失败）」；`portChanges` 非空时提示「请重启这些应用」；`relay.failOpen=true` 时固定提示「上游探测失败会改走直连」（§8.1 P9） |

- 凭据边界不变：UI 不显示用户名/密码，只显示 `hasCredentials`（egress-contract §7.1）。
- `restartRequiredApps` 非空时 WebUI 必须给可见提示（Toast/横幅），不允许静默吞掉。

---

## §7 失败与降级

### 7.1 绑定失败（启动 / 重建时）

| 端口类型 | 绑定或持久化失败的处理 |
| --- | --- |
| **设备级**（`base_port`） | **不写** `settings` 指向中继（清成 `:0`）+ `egress_relay_bind_failed`（`scope:"device"`）。**决策 #1 的落地**：宁可「设备级不启用代理」，也不做「settings 指向一个没人监听的端口」 |
| **应用级** | 该端口不监听 + `egress_relay_bind_failed`（`scope:"app"`）；其它端口照常；`local_port` 保留（不重分配，避免抖动）；引用它的应用得到「连接被拒」（§7.4） |

- **绑定面断言失败**（`getsockname` 不是 `AF_INET` / `127.0.0.1`，`reason="not_loopback"`）按绑定失败处理：关闭 socket、不监听、不写 `settings` 指向它（§5.1）。
- 端口表的写回失败（`persist_failed`）视同绑定失败：**文件与真实端口必须一致**，否则注入侧会拿着旧端口去连。
- 池耗尽（99 个都不可用）→ `reason="pool_exhausted"`、`port=null`，该定义不监听（同样不阻塞其它定义）。

### 7.2 listener 中途退出（运行期）

| 项 | 行为 |
| --- | --- |
| 检测 | accept 循环返回致命错误 / 监听 socket 被关闭 / 端口被外部抢占 |
| 状态 | `listeners[].listening=false`、`lastError={code:"egress_relay_bind_failed", reason:"listener_exited", at}`；`GET /egress` 的 `warnings` 带同名警告 |
| `settings` | **不动**（避免抖动）；只有 `via="upstream"` 的探测才可能触发 fail-open（§7.4） |
| 自动重试 | 有限次退避重绑（1s / 5s / 15s，最多 3 次）；仍失败就停在「失败 + 可见」，不无限重试、不刷日志 |
| 修复路径 | `POST /egress/relay/restart`（或服务重启 / `service.sh` 守护拉起） |
| 已完成连接 | 不受影响（只有 listener 死了，既有隧道继续跑到空闲超时） |

### 7.3 防火墙失败

- **本轮需要重建**的应用级端口不监听（未变 listener 保持监听，规则可能为空、由 accept 期 uid 双检兜底，见 §5.5 补充 B）；`GET /egress.relay.firewall.applied=false` + `egress_relay_firewall_failed`。
- 设备级 listener 照常工作（§0 D2）；`settings` 仍写 `127.0.0.1:base_port`。
- 修复：装/修 `iptables`、确认 `-m owner` 可用、`POST /egress/relay/restart`。
- 安全语义：应用级端口宁可不提供，也不裸奔（否则同机任意应用可白用凭据出网）。

### 7.4 fail-open 边界（relay 语境）

egress-contract §3.5 的三时点判定**不变**；本增量只补充「relay 会不会自己改 `settings`」：

| 情形 | 是否动设备级 `settings` | 说明 |
| --- | --- | --- |
| 启动/重建时设备级端口绑定失败 | **是**（清成 `:0`） | §7.1；决策 #1 |
| 设备级 listener 运行期中途退出 | **否**（保持现状） | 等下一次 **`via="upstream"`** 的显式探测（`reason=connect`）走既有 fail-open 切直连；不做后台健康检查（不加定时器、不加流量） |
| 探测 `via="upstream"` 失败 | 是（仅 `reason ∈ {connect,dns,timeout,tls}`） | 既有三时点判定不变；`[probe].fail_open=false` 时不切（§8.1 P9） |
| 探测 `via="relay"` 失败（任何 reason，含 `timeout`） | **否** | 中继死时绝不许把设备级切直连——否则 fail-closed 变成 IP 泄漏（memo R2/R9） |
| 上游不可达 / 凭据错（数据面） | **否** | 只影响该连接的请求（502/407），不切链路 |
| 应用级 relay 失败 | **否** | 应用级本就不改设备级设置（egress-contract §1.6「应用级失败不降级」不变） |
| 应用级 relay 失败是否回退注入上游 | **否**（静态注入） | companion 无 IPC，注入值只看文件（§3.2 不变量）；失败表现为「连接被拒」 |
| `[relay].enabled=false` | 是（回上游 `host:port` 或 `:0`） | 这是**配置选择**，不是 fail-open |
| `__DIRECT__` | 永远直连 | 与 relay 无关 |

**一句话边界**：relay 只在「自己绑不上」这一件事上主动让设备级退回直连；探测触发的 fail-open 是**既有 egress 机制**，且只有 `via="upstream"` 能触发它（`via="relay"` 永不触发）；其余一切故障都只让**受影响的链路**失败并可观测，绝不静默改出口。

### 7.5 上游不可达 / 认证失败（数据面与告警）

- 数据面：`502`（HTTP）/ `REP=0x01`（SOCKS5），一行日志（§4.8）。
- 告警：`egress_relay_upstream_unreachable`（§6.4），随该 listener 下一次握手成功自动消失。
- **凭据错误不单独建码**：上游 `407` 与 SOCKS `auth_failed` 都走 `reason="auth_failed"`；不新增错误码（与 egress-contract §6.2「错误码不变」一致）。
- 凭据值永不进日志/响应/审计/WebUI（egress-contract §7.1 + §4.8）。

### 7.6 状态与观测（三处，缺一不可）

1. `GET /egress.relay`：`enabled` / `listeners[]` / `firewall`（`applied` 由 `-S` 现算）/ `portChanges`（写响应）/ `restartRequiredApps`（写响应）。
2. 服务日志：`relay listen` / `relay listener exited` / `relay upstream ... failed` / `relay firewall ...` / `relay port reallocated`（都不含凭据）。
3. 人工/QA：`ss -ltn`（只回环）、`ss -ltn6`（必须为空）、`iptables -S DF_EGRESS_RLY` / `-S DF_EGRESS_RLY_ALLOW` / `-S DF_EGRESS_RLY_DENY`（三链）、`settings get global http_proxy`（`127.0.0.1:17800` 或 `:0`）。

`relay` 状态**只在内存**（进程重启清零、按文件重建），不落盘（与 `health`/`lastProbe` 同口径；设备端没有数据库）。

---

## §8 可检出性代价与文档声明位置

### 8.1 代价清单（如实写）

| # | 代价 | 谁看得见 | 强度 |
| --- | --- | --- | --- |
| P1 | 多一个 `127.0.0.1:178xx` 监听端口 | 同 uid 的检测代码、root；Android 9+ 跨 uid 的 `/proc/net/tcp` 受限，普通应用看不到别的 uid 的 socket | 中 |
| P2 | 端口区间**固定**（17800–17899），可被一次性扫描 | 能看到自身 socket 的检测代码只能看到自己连的那个端口；root 扫描可全见 | 中 |
| P3 | 应用侧代理地址从「真实上游」变成 `127.0.0.1`（`http.proxyHost` / `Proxy.getDefaultHost()` 可读） | 应用进程内自检代码 | **强**（最明显） |
| P4 | 设备级 `settings global http_proxy` 变成 `127.0.0.1:17800` | 任何能读系统代理的应用（`Proxy.getDefaultHost()`） | **强** |
| P5 | `iptables` 链 `DF_EGRESS_RLY`（固定名）与规则 | 只有 root / `CAP_NET_ADMIN`；普通应用看不到 | 低 |
| P6 | 中继端口对所有 uid 可达（当 `firewall=false`） | 同机任意应用可白用凭据 | 安全代价（配置项，默认不开） |
| P7 | 中继进程常驻（与既有 `device_faker_server` 同进程，不新增进程） | 无新增可检出面 | 无 |
| P8 | **中继把「应用出网」变成「root 出网」**：任何按 uid 生效的第三方防火墙（AFWall+、NetGuard、厂商联网管控）看到的都是 root 的连接，用户「禁止该应用联网」的策略被中继绕过；`settings` 指向 `127.0.0.1` 会放大这一点 | 第三方 uid 防火墙、root | 中 |
| P9 | **fail-open 会改出口**：`[probe].fail_open` 默认 `true`，relay 生效后上游探测失败（`connect`/`dns`/`timeout`/`tls`）会清 `settings` 回 `:0` → 全设备**静默直连**。对「带认证代理 = 指定出口」的用户就是 IP 泄漏 | 出口 IP 的观察者、用户自己 | 中 |
| P10 | **设备级 socks5 配置不工作（已 fail-closed，不是暴露面）**：`default` 指向 socks5 定义 → `PUT` 直接 422；旧配置在 `enabled=true` 时 `settings` 写黑洞 `127.0.0.1:9`（`enabled=false` 按未配置/直连），并报 warning `egress_relay_device_socks5_unsupported`；设备级没有出口，`base_port` 上**没有** SOCKS5 listener | 配置了该形态的用户 | 低（有显式 422 / warning） |
| P11 | **端口释放窗口可被抢占**：listener 关闭后、重绑前，其它本机进程可抢占 178xx；应用用旧端口时会连到抢占者（§5.4.1） | 同机任意应用 | 低（毫秒级窗口，且有 `portChanges` / `restartRequiredApps` 信号） |

### 8.2 缓解（已内置）

1. **只对带凭据的代理启用**：无凭据代理、`__DIRECT__`、未配置的应用，注入值与 v1.10.0 **逐字节相同**（不写 `[egress_proxy]` 或写上游地址）。
2. **只为被引用的定义建 listener**：定义了但没人用的带凭据代理不监听（端口表有值、`relayActive=false`），可见面最小。
3. **整体开关**：`[relay].enabled=false` 回到 v1.10.0 语义（端口、监听、规则全无），供「宁可不认证也不想被检出」的场景。
4. **不新增进程/服务**：中继跑在既有 root 服务进程里（与计划 §4 一致），不新增自启项、不新增日志文件。
5. **规则只碰 178xx**：不改其它端口的连通性，不引入 INPUT/FORWARD 规则，不做流量劫持。
6. **可关**：要保证「绝不直连」，把 `[probe].fail_open` 设为 `false`（失败只报错、不动 `settings`）；`GET /egress.relay.failOpen` 暴露当前值，UI 在 relay 生效且为 `true` 时提示（§8.3）。

### 8.3 文档声明位置（t8 / t11 必须落笔）

| 文件 | 章节 | 写什么 | 语言 |
| --- | --- | --- | --- |
| `docs/CONFIG.md` | 「网络出口（egress）」→「已知限制」 | 新增 2 条：① 中继端口（`127.0.0.1:178xx`）与链名 `DF_EGRESS_RLY` 属可检出项（P1–P5，与「armed 进程常驻代码页」同级表述）；② `[relay]` 开关与代价（关掉就回到「带凭据不可用」） | zh |
| `docs/CONFIG.md` | 「已知限制」→ 追加 3 条（memo R8/R9） | ③ 中继 = **root 出网**：第三方 uid 防火墙（AFWall+/NetGuard）不再拦住该应用（P8）；④ `[probe].fail_open` 默认开 → 认证代理下探测失败会**静默直连**，要「绝不直连」请设为 `false`（P9）；⑤ **关闭中继没有 API**：编辑 0600 的 `egress.toml` 的 `[relay].enabled` + `POST /egress/relay/restart` | zh |
| `docs/CONFIG.md`、`docs/SERVER.md` | 「已知限制」→ 追加 3 条（memo R12/R13，v1.2；**三语同改**） | ⑥ **回退窗口 · 凭据载体结论（移除）**：中继明确未启用（`enabled=false` / 文件未迁移）时应用拿到上游 `host:port`；**v1.2 起不再写任何凭据载体**——「SOCKS 凭据回到应用进程、同 uid 可读」这一旧行为在 v1.2 之后不再发生（≤v1.1 会发生）；HTTP Basic 从来不可用。**备选（不采纳）**：若保留载体，只允许在显式 `enabled=false`，且必须声明「SOCKS 凭据会进应用进程属性、同 uid 可读；HTTP Basic 仍不可用」。该状态下的带认证代理只能靠 IP 白名单 | zh |
| `docs/CONFIG.md`、`docs/SERVER.md` | 同上 | ⑦ **fail-closed**：relay 开启但端口未就绪 / spec 视图非法时，应用被指向 `127.0.0.1:9`（断网），**不静默直连、凭据不进应用进程**；只覆盖 Java 层，native/WebView 不受控 | zh |
| `docs/CONFIG.md`、`docs/SERVER.md` | 同上 | ⑧ **恢复路径**：等 server reconcile / `POST /egress/relay/restart` / 重启应用；用 `GET /egress.relay.listeners[].listening` 与 `proxies[].relayActive` 判断是否恢复 | zh |
| `docs/SERVER.md` | §10.8（新增章节）追加段落 | 三条链结构与 `iptables -S` 排障、绑定面断言（`ss -ltn` / `ss -ltn6`）、`firewall=false` 与 accept 双检的关系（R16）、P8/P9 两条代价 | zh |
| `docs/CONFIG.md` | 「网络出口（egress）」→ egress.toml 小节 | `[relay]` 键表、`local_port` 只读、改完要 `POST /egress/relay/restart` | zh |
| `docs/SERVER.md` | §10.4（凭据与文件权限）后新增 §10.8「设备内中继与 iptables」 | 链名与规则、`iptables -S` 排障、`firewall=false` 的风险、卸载/禁用清理 | zh |
| `docs/SERVER.md` | §10.5（卸载与禁用清理） | 追加「禁用前清链」的命令 | zh |
| `docs/API.md` | §5.25–§5.29（egress）并新增 §5.30 | `relay` 对象字段表（含 `failOpen` / `allowRules` / `denyRules`）、`localPort`/`relayActive`、`POST /egress/relay/restart`、5 个警告码、`PUT` 的 `unknown_field`；**§5.28 探测加 `via`/`relayPort` 与 `relay_not_listening`/`relay_connect` 两个 reason**；§4 端点总表 +1 | zh |
| `docs/en/*`、`docs/tr/*` | 对应章节 | 三语同改（既有要求，见 egress-contract 附录 B） | en / tr |
| `manager/docs/API.md` | DeviceSummary / `GET /devices/{id}/egress` | `relay:{enabled,active}` 与 `deviceState.relay` 透传口径 | zh |
| `CHANGELOG.md`（+ en/tr） | 新增版本条目 | 「带用户名/密码的代理从『不可用』变为『经设备内中继可用』」+ 已知限制指针 | 三语 |

> 「已知限制」的既有措辞（「带用户名 / 密码的代理本期不可用」「等 P2 设备内本地认证中继」）必须**改写**，不能只是追加：否则文档会同时说「不可用」和「可用」。改写口径：`[relay].enabled=false` 或文件未迁移时退回上游 `host:port`（**不再写凭据载体**，v1.2；给 `egress_auth_unsupported`）；relay 开启但端口未就绪 / 视图非法时 **fail-closed 断网**；其余情况经中继生效。
>
> **⑥⑦⑧ 三条必须三语同改**（`docs/CONFIG.md`、`docs/SERVER.md` 与 `docs/en/*`、`docs/tr/*` 对应章节），并同步 `CHANGELOG.md`（三语）。

---

## §9 与 egress-contract.md 的差异与冲突

### 9.1 冲突项（3 条，全部有替代规则）

| # | egress-contract 原文 / 位置 | 本契约 | 冲突 | 理由 |
| --- | --- | --- | --- | --- |
| K1 | §5.1 第 7 项 + §8 #7：`hasCredentials` 时必须注入凭据载体（`java.net.socks.username`/`password`、`Authenticator`）；实测不成立，降级为文档限制 | **v1.2 起任何路径都不注入凭据载体**：relay 生效时认证由中继承担；回退窗口（`enabled=false` / 文件未迁移）也只注入上游 `host:port`（§3.2 规则 5、§9.2 B13）。`egress_auth_unsupported` 保留，语义收窄为「该代理在当前状态下不走中继、认证不可用」 | **是**（行为替代，范围比 v1.1 更大） | 凭据写进应用进程（同 uid 可读）只增加暴露面、不增加可用性（memo R13）；中继是唯一会出示凭据的地方 |
| K2 | §5.1 说明：「只操作 **Java 层**……**不写 iptables**」 | 本增量**新增** `iptables` **链族**：入口 `DF_EGRESS_RLY` + 可重建 `DF_EGRESS_RLY_ALLOW` + 静态兜底 `DF_EGRESS_RLY_DENY`（只针对 178xx，§5） | **是**（范围扩大） | uid 隔离是「不把设备变成开放代理」的唯一可行手段（计划 §5.1）。原句描述的是 **v1.10.0 应用级注入**的作用域，不是 relay 的边界；文档收口时改成「注入侧不写 iptables；uid 防火墙属设备级 relay 实现」 |
| K3 | §3.7 第 2 条 + §8 #7：「设备级**不承载认证**（`settings` 只写 `host:port`）；指向带凭据的代理时给 `egress_credentials_ignored`」 | relay 生效时设备级**承载认证**（`settings` 写 `127.0.0.1:base_port`，中继补 Basic/RFC1929），**不再**产生 `egress_credentials_ignored`；`enabled=false` 时回到旧行为 | **是**（限制解除） | 决策 #1（设备级也走中继）。`egress_credentials_ignored` 保留但收窄为「relay 关闭时的设备级带凭据」 |

### 9.2 补足项（不改既有语义，只是把空白填上）

| # | egress-contract 未定义 | 本契约 | 冲突 |
| --- | --- | --- | --- |
| B1 | `PUT /egress` 幂等比较是否包含服务端派生字段 | 排除 `local_port` 与 `[relay]`（§1.8） | 否 |
| B2 | `egress.toml` 被 server 写端口后 revision 会推进 | 明确写进 §1.8，客户端按既有 412 处理 | 否（§1.2 算法不变） |
| B3 | `exclusionList` 与中继的相互作用 | 中继生效时禁止排除回环（`422 invalid_value`，§3.1） | 否（新增校验） |
| B4 | 探测是否经中继 | **两条路径**：`via="upstream"`（缺省，直连上游）与 `via="relay"`（经本地中继，§3.5）；只有 upstream 失败可触发 fail-open | 否（补足；v1.1 修订，原「不经中继」的绝对表述已删除） |
| B5 | 数据面 502/400/503 与 API 错误码的关系 | 数据面结果不进错误码表（§4.7） | 否 |
| B6 | `GET /egress` 新增对象是否需要新错误码 | 不需要；relay 的失败全部用 5 个警告码表达（§6.4） | 否 |
| B7 | 端口表是否需要「清理孤儿端口」 | 删除定义/清空凭据即释放；`[relay].enabled=false` 只停不删（§1.6） | 否 |
| B8 | `local_port` 是否进 `diff` / 备份 / 导出 | 不进 diff、进备份（0600）、不进导出（§1.8/§2.4） | 否 |
| B9 | 中继的 uid 归属（单 uid 限流）如何取得 | `/proc/net/tcp` 的 `uid` 列（§4.6） | 否 |
| B10 | 端口固定区间的可检出性 | 如实写入 §8 + 三语已知限制 | 否（决策 #6） |
| B11 | `POST /egress/probe` 的路径选择与新增字段 | 请求 `via`（`upstream`/`relay`）、响应 `relayPort`、响应 `via` 值域 +`"relay"`、`reason` +`relay_not_listening`/`relay_connect`（§3.5/§6.6）；**判定顺序 409（enabled/凭据）→ 502（listening）** | 否（新增字段，旧客户端忽略） |
| B12 | `relay` 对象是否暴露 fail-open 与分链计数 | 新增 `relay.failOpen` / `firewall.allowRules` / `firewall.denyRules`（§6.1） | 否（只读派生） |
| B13 | 回退到上游的窗口与凭据载体 | 只允许「中继明确未启用」：`enabled=false` 或文件未迁移（无 `[relay]` 键且无任何 `local_port`）；回退**不写凭据载体**（v1.2，R13）。`enabled=true` 且已迁移但缺 `local_port` → fail-closed `127.0.0.1:9`（§3.2 规则 7） | 否（明确未启用时仍回退，保留可用性） |
| B14 | `restart` 与「幂等短路」的关系、`firewall.applied` 的来源 | `POST /egress/relay/restart` **强制 apply**（跳过短路）；短路判定必须含 `iptables -S` 三链校验；`applied` 由 `-S` 推出（§5.4 第 1 步、§6.1）（R14） | 否（补足，修 v1.1 的自相矛盾） |
| B15 | 非法 relay 视图的注入形态 | 哨兵 `127.0.0.1:9`（`RELAY_FAIL_CLOSED_PORT`），经 `injection_plan` 注入并 arm replay；`type=direct` + `relay=true` 按主动直连处理（§3.3）（R12） | 否（新增内部契约，不进 API） |

### 9.3 受影响的既有条目（t8 文档收口清单，不改数值）

| egress-contract 位置 | 需要改什么 |
| --- | --- |
| §1.3 字段表 | 追加 `relay`；`proxies[]` 追加 `localPort`/`relayActive` |
| §1.4 请求体表 | 追加「`localPort`/`local_port`/`relay*` → 422 unknown_field」；幂等条目追加「排除 server 拥有字段」 |
| §1.8 警告码表 | 追加 5 个 relay 码；`egress_credentials_ignored`/`egress_auth_unsupported` 的触发条件加「relay 未生效」前缀 |
| §1.9 幂等矩阵 | 追加 `POST /egress/relay/restart` |
| §1.7 `GET /egress/proxies` | 追加 `localPort`/`relayActive` |
| §2.1 `egress.toml` | 追加 `[relay]` 与 `local_port` |
| §2.2 权限 | 追加「`local_port` 写回走同一原子写路径」 |
| §3.7 已知限制 | 第 2 条改写（见 K3） |
| §1.6 探测契约 | 请求加 `via`、响应加 `relayPort`、`via` 值域加 `"relay"`、`reason` 加 `relay_not_listening` / `relay_connect`（§3.5） |
| §4.4 `reason` 枚举 | 同上两项（只增不改） |
| §5.1 / §5.2 / §5.5 | 注入清单收窄 + `[egress_proxy]` 的 `relay` 键（K1、§3.3） |
| §5.1 第 7 项（凭据载体） | v1.2 起**任何路径都不注入**（含回退窗口），文档同步（§9.1 K1、§8.3） |
| 附录 B 版本收口 | 设备端端点 29 → **30**；设备端版本 1.10.0 → **1.11.0**（建议）；`CAPABILITIES` 不变；三语文档 + `manager/docs/API.md` |

> 版本号建议只作参考（t8 决定）：本增量是**可选能力**，`/api/v1` 前缀不变，按既有 semver 习惯走 minor 递增。

---

## §10 下游取用映射与验收命令

### 10.1 任务映射

| 任务 | 必读章节 |
| --- | --- |
| t2 设备端 relay 核心（HTTP + 端口 + listener 生命周期） | §1 全部（**§1.3 分配=绑定、§1.5 先关后开**）、§2 全部、§3.1、§4.1、§4.3、§4.5、§4.6、**§5.4 生命周期与重建算法**、§6.1–§6.3、§7.1–§7.2 |
| t3 uid 防火墙 | §5 全部（**必须按 §5.1 三链 + §5.4 五步生命周期**，v1.1 已改为 fail-closed）、§4.6（uid 归属 + accept 双检）、§6.1（`firewall`/`listeners`）、§7.3 |
| t4 Zygisk 注入改造 | §3.2（规则 5–7）、§3.3（**v1.2 fail-closed 规则 1–8**）、§3.4、§9.1 K1、§9.2 B13/B15 |
| t5 SOCKS5 | §4.2、§4.4、§4.7、§5.4（listener 生命周期） |
| t6 Manager 透传 + socks5 探测 | §6.1（`relay`）、§6.2（`localPort`/`relayActive`）、§6.7 |
| t7 / t8 双侧 WebUI | §6.1、§6.2、§6.7、§7.6 |
| t9 真机验收 | **§5.7 五组命令（v1.2：协议级判据、本机可执行）**、§10.2、§10.3、§10.4（含 #19–#23）、§7.6 |
| t10 / t12 评审与集成收口 | §9 全部、**§5.4 无裸奔论证**、§7.4、§8.1（P8/P9） |
| t11 文档/契约收口 | §8.3、§9.3 |

### 10.2 设备端冒烟（t9 用；**在主机执行**）

> 设备上**没有 `curl`**；所有 API 调用在主机执行，先 `adb forward tcp:8686 tcp:8686`（**不要** `adb reverse`：本机不稳定，首个请求可达、后续丢包）。fixture 上游也不要用 `127.0.0.1` 指代，改用与设备同 /24 的 LAN IP（实测 `192.168.30.149`）；relay 语义与上游地址无关。

    adb forward tcp:8686 tcp:8686
    BASE=http://127.0.0.1:8686/api/v1
    TOKEN=admin123

    # 1) 读现状（含 relay）
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress" | jq '{revision, relay}'

    # 2) 写一个带凭据定义 + 设为设备级默认（会分配 local_port = base_port）
    REV=$(curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress" | jq -r .revision)
    curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H "If-Match: \"$REV\"" -H 'Content-Type: application/json' \
      -d '{"default":"hk_resi","proxies":[
            {"name":"hk_resi","type":"http","host":"203.0.113.10","port":8080,"username":"u","password":"p"},
            {"name":"jp_socks","type":"socks5","host":"198.51.100.20","port":1080,"username":"u2","password":"p2"}
          ]}' "$BASE/egress" \
      | jq '{changed, revision, relay}'

    # 3) 只读字段：localPort 被拒（422）
    curl -s -o /dev/null -w '%{http_code}\n' -X PUT -H "Authorization: Bearer $TOKEN" \
      -H "If-Match: \"$(curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress" | jq -r .revision)\"" \
      -H 'Content-Type: application/json' \
      -d '{"proxies":[{"name":"hk_resi","type":"http","host":"203.0.113.10","port":8080,"localPort":17801}]}' \
      "$BASE/egress"

    # 4) relay 状态与端口
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress" | jq '.relay.listeners'
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress/proxies" | jq '.items[]|{name,localPort,relayActive}'

    # 4b) 探测对照：via=upstream（缺省）与 via=relay（§3.5）
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"via":"upstream"}' "$BASE/egress/probe" | jq '{ok,via,exitIp,relayPort}'
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"via":"relay"}'    "$BASE/egress/probe" | jq '{ok,via,exitIp,relayPort}'
    # 把端口弄成不监听后：via=relay 应报 relay_not_listening，而 via=upstream 仍绿、settings 不变
    adb shell su -c 'settings get global http_proxy'

    # 5) 重建（幂等）
    curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d '{}' \
      "$BASE/egress/relay/restart" | jq '{restartedAt, relay}'

    # 6) 端口表落盘
    adb shell su -c 'grep -n "local_port\|relay" /data/adb/device_faker/config/egress.toml'

### 10.3 设备侧断言（t9 用）

    # settings 指向中继（设备级）
    adb shell su -c 'settings get global http_proxy'                    # 127.0.0.1:17800
    adb shell su -c 'dumpsys connectivity | grep -i "global proxy"'      # Global proxy: [127.0.0.1] port: 17800

    # 文件权限与端口表
    adb shell su -c 'ls -l /data/adb/device_faker/config/egress.toml'    # -rw------- root root
    adb shell su -c 'grep -c local_port /data/adb/device_faker/config/egress.toml'

    # 绑定面：只监听 127.0.0.1（IPv4），IPv6 侧为空；非回环目的地连不上（§5.7 第 1 组）
    adb shell su -c 'ss -ltn  | grep -E ":178[0-9]{2}"'
    adb shell su -c 'ss -ltn6 | grep -E ":178[0-9]{2}"'
    adb shell 'toybox nc -w 2 ::1 17831 </dev/null; echo rc=$?'            # 非 0
    adb shell 'toybox nc -w 2 127.0.0.2 17831 </dev/null; echo rc=$?'      # 非 0
    adb shell 'toybox nc -w 2 <LAN_IP> 17831 </dev/null; echo rc=$?'       # 非 0

    # 防火墙三链与顺序（完整验收见 §5.7 第 1–5 组）
    adb shell su -c 'iptables -S DF_EGRESS_RLY'
    adb shell su -c 'iptables -S DF_EGRESS_RLY_ALLOW'
    adb shell su -c 'iptables -S DF_EGRESS_RLY_DENY'

    # uid 授权：拒绝用例**不加 su**（adb shell 是 uid 2000）；放行证据只能用**应用内探针**
    # （setpriv 本机不存在；run-as 出网被 SELinux 拦 —— 见 §5.7 第 2 组）

    # 数据面协议级探针（设备无 curl；-q 必带，否则 nc 拿不到回包就退出）
    # HTTP 中继端口（absolute-URI 请求）
    adb shell 'printf "GET http://checkip.amazonaws.com/ HTTP/1.1\r\nHost: checkip.amazonaws.com\r\n\r\n" | toybox nc -q 3 -w 6 127.0.0.1 17800 | head -n 1'   # 期望：HTTP/1.1 200 或 407
    # SOCKS5 中继端口（问候 + CONNECT + 远程 DNS）
    adb shell 'printf "\005\001\000\005\001\000\003\021checkip.amazonaws.com\000\120" | toybox nc -q 3 -w 6 127.0.0.1 17831 | od -An -tx1 | head -n 2'   # 期望：05 00 ... 05 00 ...
    # 注：以上以 root 执行，只证明「中继数据面可用」；证明 uid 授权必须用应用内探针（§3.5 第 6 条）

    # 凭据零泄漏（跑完增删改 + 若干请求后）
    adb shell su -c 'grep -Eci "password|Proxy-Authorization" /data/adb/device_faker/logs/*.log'   # 0
    adb shell su -c 'grep -ci "password" /data/adb/device_faker/logs/api-audit.jsonl'              # 0
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress" | jq -e 'tostring|test("password")|not'
    curl -s -H "Authorization: Bearer $TOKEN" "$BASE/egress/proxies" | jq -e '[.items[]|has("password")]|any|not'

### 10.4 关键验收点（与计划书 §7.2/§7.3 对齐）

| # | 验收点 | 判据 |
| --- | --- | --- |
| 1 | 应用级 http 带凭据 | relay 注入后上游 200；直接连上游（不带 `Proxy-Authorization`）→ 407 |
| 2 | 设备级带凭据 | `settings` 指向 `127.0.0.1:17800`，经 relay 出网成功 |
| 3 | 上游 407（凭据错） | 应用收到 407；日志/响应无凭据 |
| 4 | 上游不可达 | 应用收 502；`egress_relay_upstream_unreachable` 出现 |
| 5 | 非授权 uid | **不加 su**（uid 2000）连应用级端口被拒（§5.7 第 2 组） |
| 6 | `__DIRECT__` | 设备级指向 relay 时该应用仍直连 |
| 7 | 无凭据代理 | 仍直连上游，**不**经过 relay（`relay.listeners` 里没有它） |
| 8 | 卸载/禁用 | 端口关闭、链清空、`settings` 清直连 |
| 9 | 凭据零泄漏 | server.log / audit / 全部 API 响应 / relay 日志命中 0 |
| 10 | 端口稳定 | 连续两次 restart：`local_port` 不变、`portChanges:[]` |
| 11 | 端口重分配 | 手工把两个定义改成同一 `local_port` → restart 后其一重分配、`restartRequiredApps` 非空 |
| 12 | 防火墙失败 | 临时改坏 `iptables` 路径 → 应用级端口不监听 + warning，设备级仍可用 |
| 13 | 重建零空窗（R1/R15） | 未授权 uid 持续发 SOCKS 问候 + 20 次 restart → **没有一次拿到中继服务（协议级）**：全程无 `05 00` 回包、且日志有 `uid_not_allowed`（§5.7 第 3 组） |
| 14 | 绑定面（R3/R4） | `ss -ltn` 全 127.0.0.1、`ss -ltn6` 空、`::1` / `127.0.0.2` / `<LAN_IP>` 三种目的地都连不上 |
| 15 | uid 失败口径（R4/R5） | 手工 `-F` 两条链后，未授权 uid 仍被 accept 期双检 close；不存在「归属失败也放行」 |
| 16 | 探测对照（R5） | `via=relay` 绿且出口 IP 等于上游；把端口弄挂 → `relay_not_listening` 且 `settings` 不变 |
| 17 | 端口稳定（R6/R7） | 连续两次 restart：`local_port` 不变、`portChanges:[]`、`restartRequiredApps:[]`、ALLOW 规则逐字节相同 |
| 18 | 代价声明（R8/R9） | 三语已知限制含 P8（root 出网绕过 uid 防火墙）/ P9（fail-open 改出口）与「关闭中继需编辑文件 + restart」 |
| 19 | fail-closed 注入（R12） | 伪造 spec：host 非 127.0.0.1 / port 池外 / type 非法 → logcat 出现 `relay fail-closed ... reason=invalid_target`，应用断网（`127.0.0.1:9`），**不**回退上游、不写凭据；`ss -ltn` 确认 9 端口无监听 |
| 20 | 回退收窄（R13） | `enabled=false` 或文件未迁移 → 上游 `host:port`（**无凭据载体**）；`enabled=true` 且已迁移但缺 `local_port` → 断网（`127.0.0.1:9`），**不**回退 |
| 21 | restart 强制 apply（R14） | 手工 `-F` 两链 → `GET /egress.relay.firewall.applied=false` → restart → `-S` 恢复、`applied=true`、未授权 uid 仍被拒 |
| 22 | 先检后读（R15） | 拒绝用例的探针**收不到 `05 00`**（协议级），且日志有 `uid_not_allowed`；证明双检在读首字节之前 |
| 23 | 本机可执行性（t9 实测） | 验收命令不依赖设备 `curl` / `setpriv` / `run-as` 出网；API 走 `adb forward`，设备侧探针走 `toybox nc -q`，fixture 上游用 LAN IP |

---

## 附录 A 变更记录

| 版本 | 日期 | 变更 |
| --- | --- | --- |
| relay contract v1.2 | 2026-12-02 | 按 [relay-review-memo.md](relay-review-memo.md) §5/§6（R12–R16）修订：① **R12 fail-closed 注入**：非法 relay 视图不再按「未配置」返回，改为哨兵 `127.0.0.1:9`（`RELAY_FAIL_CLOSED_PORT`）经 `injection_plan` 注入并 arm replay，日志 `relay fail-closed ... reason=invalid_target` / `reason=port_missing`；唯一例外（仅多带凭据）剥凭据照常注入；`type=direct` + `relay=true` 按主动直连；残留风险（只覆盖 Java 层）写明；② **R13 回退收窄**：只有「中继明确未启用」（`enabled=false` 或文件未迁移）才回退上游，且**回退也不再写凭据载体**（§9.2 B13）；`enabled=true` 且已迁移但缺端口 → fail-closed；③ **R14 restart 强制 apply**：restart 跳过 §5.4 短路，短路判定必须含 `iptables -S` 三链校验，`firewall.applied` 由 `-S` 推出；④ **R15 验收判据协议化**：§5.7 第 2/3 组与 §10.4 #13 改为「收不到 `05 00` / 日志 `uid_not_allowed`」，并写明「先检后读」；⑤ **R16**：`firewall=false` 不再声称「全 uid 可达」（accept 期双检仍生效）；⑥ 按 t9 实测改写 §5.7/§10.2/§10.3 为**本机可执行**版本（设备无 curl/setpriv、run-as 出网被 SELinux 拦、fixture 上游用 LAN IP、API 走 `adb forward`） |
| relay contract v1.1 | 2026-12-01 | 按 [relay-review-memo.md](relay-review-memo.md) R1–R11 修订：① 防火墙改 **fail-closed 三链**（入口 + 可重建 ALLOW + 静态 DENY），给出「无裸奔」论证与 listener 五步生命周期（R1/R2/R7）；② bind 面硬断言 `getsockname` + 五项验收（R3/R4）；③ uid 失败口径统一为 fail-closed（规则期不监听 + accept 期双检），消除 §4.6/§4.8 矛盾（R5/R11）；④ 探测加请求字段 `via`（`upstream` / `relay`）、响应 `relayPort`、`relay_not_listening`/`relay_connect`，并禁止 relay 失败触发 fail-open（R2/R9）；⑤ §5.7/§10.3 验收命令重写（拒绝用 uid 2000、允许用真实 uid）（R3/R6）；⑥ 端口重建「先关后开 + 分配即持有 socket」，消除 EADDRINUSE 误重分配与 TOCTOU（R6/R10）；⑦ §8 增 P8（root 出网绕过 uid 防火墙）/ P9（fail-open 改出口）与关闭通道说明（R8/R9） |
| relay contract v1 | 2026-11-25 | 首版冻结：端口分配（17800–17899、设备级占 `base_port`、FNV-1a + 线性探测）、`egress.toml [relay]` 与只读 `local_port`（0600）、设备级/应用级三条生效优先级、数据面（本地 HTTP CONNECT/absolute-URI + 本地 SOCKS5 `0x00` + 上游 Basic/RFC1929、32 KiB 头、10s/120s、256/64）、uid 防火墙（`DF_EGRESS_RLY`、`pkg@uid`、失败不监听、幂等重建与卸载清理）、API（`relay` 对象、`localPort`/`relayActive`、`POST /egress/relay/restart`、警告码（首版 4 条，v1.3 起 5 条）、`local_port` → 422）、失败与降级、可检出性代价与三语声明位置、与 egress-contract 的 3 条冲突与 10 条补足 |
