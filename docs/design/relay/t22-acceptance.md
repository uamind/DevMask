# t22 复验（round 3）：带认证中继 —— Pixel 9

- 结论：**PASS**（captain 两条新判据 + #13/#21 + 应用级 SOCKS5/远程 DNS 全部达成）。
- 构建：t21 合并态（server 4216040 B / zygisk 1448528 B），Pixel 9 / Android 17 / KernelSU；契约 v1.2。
- 证据：`docs/design/relay/evidence-t22/`（verify-commands、f2-http、f2-socks-clean、firewall-false-failclosed、21-forced-apply、uid-deny-allow、bind-face、credential-scan、cleanup-restore、final-state）。

## 1. 新判据 1：applied=true 且应用级 listener accept

`PUT /egress`（dev_http 17800 + app_http 17835，firewall=true）后：

    relay.firewall = {"applied": true, "rules": 9, "allowRules": 4, "denyRules": 2, "lastError": null}
    listeners: dev_http 17800 listening=true allowedUids=[]；app_http 17835 listening=true allowedUids=[0,10325]
    warnings: []

真机 `iptables -S`（语义比较已能吸收选项顺序与隐式 `-m tcp`）：

    -A DF_EGRESS_RLY_ALLOW -d 127.0.0.0/8 -o lo -p tcp -m tcp --dport 17800 -m owner --uid-owner 0 -j ACCEPT
    -A DF_EGRESS_RLY_ALLOW -d 127.0.0.0/8 -o lo -p tcp -m tcp --dport 17835 -m owner --uid-owner 0 -j ACCEPT
    -A DF_EGRESS_RLY_ALLOW -d 127.0.0.0/8 -o lo -p tcp -m tcp --dport 17800 -j ACCEPT
    -A DF_EGRESS_RLY_ALLOW -d 127.0.0.0/8 -o lo -p tcp -m tcp --dport 17835 -m owner --uid-owner 10325 -j ACCEPT
    -A DF_EGRESS_RLY_DENY -p tcp -m tcp --dport 17800 -j REJECT --reject-with icmp-port-unreachable
    -A DF_EGRESS_RLY_DENY -p tcp -m tcp --dport 17835 -j REJECT --reject-with icmp-port-unreachable

应用内探针（uid 10325）经 17835：`fetch.app=status=200 body=198.51.100.8`、`fetch.ip=200`；
fixture 侧 `listener=18082 GET http://app.fixture/ ... auth=ok digest=7213dc66f53c`（凭据只出现在上游握手）。

## 2. 新判据 2：uid 归属两种态

| 态 | 观测 | 结论 |
| --- | --- | --- |
| `firewall.applied=true` + uid 未知 | server.log `relay uid 归属失败但防火墙已生效：放行（内核层已按 uid 过滤） proxy=app_http port=17835 reason="uid_unresolved_firewall_enforced"`；应用探针 200 | **放行符合策略** |
| `firewall=false` + uid 未知 | 两 listener listening、allowedUids=[0,10325]，server.log `uid="unknown" reason="uid_unresolved"`；探针 `fetch.app=SocketException` | **fail-closed 符合策略** |

（两种态切换用「改 egress.toml 的 `[relay] firewall` + POST /egress/relay/restart`」，不用 PUT。）

## 3. 应用级 SOCKS5 + 远程 DNS

    EGRESSPROBE: socksProxyHost=127.0.0.1, socksProxyPort=17864
    EGRESSPROBE: selector.app.fixture=SOCKS @ 127.0.0.1/<unresolved>:17864
    EGRESSPROBE: fetch.app=status=200 body=198.51.100.8
    # fixture（RFC1929 必需）
    proto=socks5 listener=18081 method=userpass(ok) CONNECT app.fixture:80        <- 域名，未本地解析
    proto=socks5 listener=18081 tunnel-request app.fixture 164 bytes auth_header=0 cred_hit=0
    digest=c71343023d5c（sha256("t9suser:t9spass-ro3X")[:12]）

环境说明：本轮 reboot 后设备换到 192.168.100.18/24（原 fixture 主机 192.168.30.149 不可达），
因此 SOCKS5 这组改走 `adb reverse tcp:18080/18081`（本轮可用）；HTTP 组在换网前用 LAN 完成。

## 4. #21 强制 apply 与 #13 竞态

- **#21**：手工 `-F` ALLOW/DENY → `applied=false, reason=state_mismatch`（期间 uid 2000 协议级无 `05 00`）→
  `POST /egress/relay/restart` → `applied=true, rules=10`、三链齐全（entry 3 / ALLOW 4 / DENY 3）、listener 仍 listening。
- **#13**：设备内**单条 while 循环**（150 次）持续发 SOCKS 问候，主机连打 20 次 restart：

      ==== 循环脚本（/data/local/tmp/hitloop.sh，设备内单进程） ====
      i=0; h=0
      while [ $i -lt 150 ]; do
        out=$(printf '\005\001\000' | toybox nc -q 1 -w 2 127.0.0.1 17835 2>/dev/null | od -An -tx1)
        if echo "$out" | grep -qw 05; then echo "HIT i=$i out=$out"; h=$((h+1)); fi
        i=$((i+1))
      done
      echo "loops=$i hits=$h"
      ==== 结果 ====
      loops=150 hits=0          （协议级判据：整行/word 匹配 05，未用子串）

  日志交叉证据：`uid_not_allowed=0`、`uid_unresolved=14` —— 原因是被测 uid 2000 在**内核 iptables 层**就被
  DENY 的 REJECT 挡掉（没有到达 accept 期双检），这正是「不靠 accept 双检也不裸奔」的预期形态；无一次拿到 `05 00`。

## 5. 回归与清理

- **无凭据代理**：`plain_http` 不进 relay；探针属性直指上游 `192.168.30.149:18080`，fixture `auth=missing rejected=407`。
- **__DIRECT__**：属性全 null、`selector.app.fixture=DIRECT`、`fetch.app=UnknownHostException`。
- **bind 面**：`ss -ltn` 仅 127.0.0.1；`ss -ltn6` 空；`::1` / `127.0.0.2` / `192.168.30.2` 全部 Connection refused。
- **凭据零泄漏**：5/5 路扫描命中 0（device server.log / api-audit / GET egress / GET proxies / fixture log）。
- **验收开始 DELETE /egress**：`If-Match: *` → 200；warnings 按 code 过滤。
- **设备恢复**：`settings=:0`、`DF_EGRESS_RLY` 链不存在、178xx 无监听、adb reverse 已撤、fixture 已停、server 正常运行。

## 6. 观察（非阻塞，供 t10 评审）

- **O1**：accept 期 uid 归属在本机仍 `uid_unresolved`（规则期解析正常）；t21 的「firewall.applied=true 时放行」策略覆盖了可用性，
  但「按 uid 精确归属」仍未在真机成立——若后续要求严格归属，需要换 SO_PEERCRED 路径再验。
- **O2**：本轮中段做「应用代理从 app_http 切到 app_socks 再切回」时，出现过一次 `applied=false(state_mismatch)` 且
  `-S` 内容与成功态一致；手工 `-F` 两链 + restart 后恢复。干净重装 + 单次配置下不复现。建议评审关注「期望态与 uid 解析时序」。
