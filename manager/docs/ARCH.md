# Manager / Relay 架构与数据模型

> 状态：**已冻结（contract 里程碑）**
> 版本：manager 0.2.0（egress 增量：schema v2 + 代理池）
> 日期：2026-11-24
> 接口契约：[API.md](API.md)；类型定义：[api-types.ts](api-types.ts)；计划书：[../docs/plans/manager-relay-plan.md](../../docs/plans/manager-relay-plan.md)
> 上游设备端：`../../docs/API.md`（devmask_server 1.9.0）

本文冻结 Manager 的 crate 布局、SQLite 表结构、设备 Token 加密、监听与数据目录、静态托管、
以及与设备端 `/api/v1` 的映射。接口字段以 [API.md](API.md) 为准，本文只解释「怎么实现」。

---

## 1. 目标与约束

| 约束 | 设计结论 |
| --- | --- |
| 单一控制面 | 一个 Rust 进程 = Relay API + SQLite + WebUI 静态托管 |
| 默认内网安全 | 默认 `127.0.0.1:8765`；非 loopback 时必须显式设置非默认 Token，否则拒绝启动 |
| 设备 Token 隔离 | Token 只存服务端密文；API 响应、日志、错误详情、审计、前端永不出现 |
| 离线可用 | 设备列表 / 矩阵 / 总览全部读 SQLite 缓存，不因设备离线而 5xx |
| 下发可解释 | 每个 assignment 产生结构化 steps（revision / template / bind / restart）与稳定错误码 |
| 可测试 | `lib.rs` 暴露 `build_router(AppState)`，集成测试用进程内 Mock 设备 + 临时数据目录 |
| 单文件存储 | SQLite（WAL）+ 一个 32 字节密钥文件 + 可选 Token 文件 |

非目标（本增量）：多租户 / RBAC、TLS 终结、限流、定时同步、WebSocket、模板库独立存储、
多写连接池、密钥轮换、应用图标抓取。见 [API.md §1.2](API.md#12-明确不做本增量)。

---

## 2. 进程模型与模块边界

### 2.1 进程结构

    main.rs
      ├── 解析参数（args.rs）→ 合并 manager.toml / 环境变量（config.rs）
      ├── 初始化 tracing（stderr，DFM_LOG 控制级别）
      ├── 打开数据目录、密钥（crypto.rs）、SQLite（db.rs）、执行迁移与崩溃恢复
      ├── 构造 AppState（state.rs）
      ├── build_router(state)（lib.rs + api/mod.rs）
      ├── 绑定 listener（默认 127.0.0.1:8765），打印一行启动摘要（不含 Token）
      └── 监听 SIGINT / SIGTERM，优雅关闭：停止接受新连接、等待 job 句柄 ≤5 s、WAL checkpoint

### 2.2 中间件顺序（外 → 内）

1. 自定义 request-id 中间件：生成或回填 `X-Request-Id`，写入响应头与 tracing span。
2. `TraceLayer`（tower-http）：方法、路径、状态、耗时；**不**记录请求体与 Authorization。
3. `CompressionLayer`（br / gzip）。
4. `RequestBodyLimitLayer::new(1 MiB)`：超出 → `413 body_too_large`（转成错误信封）。
5. `/relay/api/v1` 子路由上的 `auth`（`middleware::from_fn`）：校验 Bearer，注入 `Caller` 扩展。
6. 业务 handler。
7. 静态路由（非 API）不经过鉴权。

### 2.3 Crate 布局（冻结）

    manager/
    ├── Cargo.toml                 # package devmask_manager, bin 同名, edition 2024
    ├── README.md                  # 使用说明（t1 更新）
    ├── docs/
    │   ├── API.md                 # Relay 接口契约（t1）
    │   ├── ARCH.md                # 本文件（t1）
    │   └── api-types.ts           # 前端类型定义（冻结）；webui/src/api/types.ts 为同步副本
    ├── src/
    │   ├── main.rs                # 参数、日志、启动、优雅退出
    │   ├── lib.rs                 # pub fn build_router(AppState) -> Router；供测试复用
    │   ├── args.rs                # --bind/--data-dir/--static-dir/--token/--config/--help/--version
    │   ├── config.rs              # manager.toml 解析、优先级合并、默认值与校验
    │   ├── state.rs               # AppState、TokenState、并发信号量、设备级串行锁
    │   ├── db.rs                  # 连接、迁移、DAO（devices/device_overviews/jobs/job_results/audit + egress_proxies/device_egress/egress_checks）
    │   ├── crypto.rs              # AES-256-GCM 加解密、devmask-manager.key 读写与权限
    │   ├── auth.rs                # Bearer 校验、Token 来源解析、POST /token/rotate
    │   ├── error.rs               # ApiError、IntoResponse、错误信封、ValidationError
    │   ├── ids.rs                 # device id slug、job id、request id、RFC 3339 时间
    │   ├── device_client.rs       # baseUrl 归一化、reqwest 调用、错误分类、overview 拉取
    │   ├── jobs.rs                # 归一化、runner、流水线、状态机、崩溃恢复、清理
    │   ├── static_files.rs        # dist 解析、SPA fallback、缓存头
    │   └── api/
    │       ├── mod.rs             # 路由表与 layer 组合
    │       ├── health.rs
    │       ├── devices.rs         # 列表/添加/详情/修改/删除/同步
    │       ├── templates.rs       # 缓存聚合
    │       ├── apps.rs            # 矩阵聚合
    │       ├── overview.rs        # 总览聚合
    │       ├── apply.rs           # 请求校验 + 入队
    │       ├── jobs.rs            # job 查询（/jobs、/jobs/{id}）
    │       ├── templates.rs       # 模板并集聚合
    │       ├── overview.rs        # 管理端总览
    │       ├── audit.rs           # 审计查询
    │       ├── token.rs           # Manager Token 轮换
    │       ├── aggregate.rs       # 设备×应用聚合快照（apps/overview/templates 共用）
    │       ├── proxy.rs           # 单设备 /api/v1 透传
    │       ├── egress.rs          # 代理池 CRUD、Manager 侧探测、绑定与批量下发入队
    │       └── egress_dto.rs      # egress 请求 / 响应 DTO（无 username / password 字段）
    ├── tests/
    │   ├── common/mod.rs          # 临时 Manager + Mock 设备启动器
    │   ├── mock_device.rs         # axum Mock 设备（带 412 / 404 / 离线开关）
    │   ├── manager_api.rs         # 设备 CRUD、sync、apps、overview、auth、静态托管
    │   ├── apply_jobs.rs          # bulk/assignments、412 重试、404、离线、restart 警告、幂等
    │   └── egress_flow.rs         # 代理池 CRUD、绑定、批量下发、探测、凭据加密
    │                              # （schema v1→v2 迁移测试在 src/db.rs 的单元测试里）
    └── webui/                     # Vue 3 管理端（frontend-dev 作用域）
        ├── src/api/client.ts      # 由 frontend-dev 新增
        ├── src/stores/manager.ts  # 改为 async
        └── ...

> 与计划书 §3.1 的差异：新增 `lib.rs`（可测试的 Router 构造）、`state.rs`、`args.rs`、`ids.rs`，
> 以及 `api/{health,templates,overview,audit,proxy}.rs`。原计划的 `api/{mod,devices,apps,apply,jobs}.rs` 全部保留，
> 只是把聚合查询与代理拆成独立文件，避免单个 handler 文件过大。此布局即为冻结结果。

### 2.4 关键类型

    pub struct AppState {
        pub cfg: Arc<Config>,
        pub db: Db,                        // Arc<Mutex<rusqlite::Connection>> + 阻塞池
        pub key: Arc<DeviceKey>,           // 32 字节 AES-256 密钥（手写 Debug 脱敏）
        pub http: DeviceHttp,              // reqwest 封装：探测 / overview / 超时与错误分类
        pub token: RwLock<TokenState>,     // 当前生效 Token + 来源
        pub started_at: SystemTime,
        pub sync_sem: Arc<Semaphore>,      // /devices/sync 并发
        pub apply_sem: Arc<Semaphore>,     // 下发 device 级并发
        pub job_sem: Arc<Semaphore>,       // 同时运行的 job 数（2）
        pub device_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    }

    pub struct TokenState {
        pub value: String,                 // 当前 Token 明文（仅内存）
        pub source: TokenSource,           // Cli | Env | File | Config | Default
    }

    pub enum ApiError { /* 见 error.rs；每个变体映射 API.md §2.7 的 code/http/retryable */ }

ID 规则：device id `^[a-z0-9][a-z0-9_-]{0,63}$`；job id `job_` + uuid v7 的 32 位小写 hex；
request id `req_` + 32 位小写 hex；时间统一 RFC 3339 UTC 秒精度字符串。

---

## 3. 依赖清单（冻结）

| crate | 版本 | features | 用途 |
| --- | --- | --- | --- |
| `axum` | 0.8 | `http1, json, query, tokio, original-uri`（关闭默认） | HTTP 服务与路由 |
| `tokio` | 1 | `rt-multi-thread, macros, net, signal, time, sync, fs` | 运行时、信号、并发 |
| `reqwest` | 0.12 | `json, rustls-tls`，`default-features = false` | 调用设备 `/api/v1` |
| `rusqlite` | 0.32 | `bundled` | SQLite（自带 amalgamation，免系统库） |
| `serde` | 1 | `derive, std` | DTO |
| `serde_json` | 1 | — | JSON、缓存 payload |
| `uuid` | 1 | `v7` | job id（时间有序，便于排序与排障） |
| `aes-gcm` | 0.10 | — | 设备 Token 加密（AES-256-GCM） |
| `base64` | 0.22 | — | 密文编码 |
| `sha2` | 0.11 | — | 聚合 revision |
| `subtle` | 2 | — | Token 常量时间比较（0.2.0 已被 yank，改用 2.x，API 相同） |
| `zeroize` | 1 | — | 解密后的设备 Token 在 drop 时清零 |
| `url` | 2 | — | `baseUrl` 解析与归一化 |
| `chrono` | 0.4 | `clock, std, serde` | RFC 3339 时间戳 |
| `tower-http` | 0.6 | `fs, trace, compression-br, compression-gzip` | 静态文件、日志、压缩 |
| `tracing` | 0.1 | — | 结构化日志 |
| `tracing-subscriber` | 0.3 | `env-filter, fmt` | 日志初始化 |
| `anyhow` | 1 | — | 启动期错误 |
| `thiserror` | 2 | — | 错误枚举 |

dev-dependencies：`tower` 0.5（`util`）、`tempfile` 3。

> `reqwest` / `url` 已在 backend-devices 任务加入（设备探测与 baseUrl 归一化）；
> backend-core 未引入当时用不到的依赖，避免空依赖与不必要的编译时间。

说明：

- 不引入 `clap`：参数只有 6 个，`args.rs` 手写 80 行即可，减少二进制体积。
- 不引入连接池（`r2d2`）：单进程低频写，一个写连接 + WAL 足够；见 §5.1。
- 不引入 `directories`：数据目录按平台 `env` 推导（§4.1），规则明确即可。

---

## 4. 数据目录与配置文件

### 4.1 数据目录

默认解析顺序（先命中先用）：

| 顺序 | 来源 |
| --- | --- |
| 1 | `--data-dir <PATH>` |
| 2 | `DFM_DATA_DIR` |
| 3 | 平台默认：Linux / 其它用 `$XDG_DATA_HOME/devmask-manager`（未设置则 `$HOME/.local/share/devmask-manager`）；macOS 用 `$HOME/Library/Application Support/devmask-manager`；Windows 用 `%LOCALAPPDATA%\devmask-manager` |

目录内容：

    <data-dir>/
    ├── devmask-manager.db          # SQLite 主库（运行时伴随 devmask-manager.db-wal / devmask-manager.db-shm）
    ├── devmask-manager.key         # 32 字节 AES-256 密钥，权限 0600，首次启动生成
    └── manager.token       # 可选：POST /token/rotate 写入的 Token 明文，权限 0600

- 目录权限始终收敛为 0700（Unix，已存在也会收紧）；数据库文件与 `-wal`/`-shm` 尽力设为 0600。
- 数据目录只允许本进程读写；日志与错误信息只出现相对文件名，不出现绝对路径。
- 备份 = 停服后复制 `devmask-manager.db` **与** `devmask-manager.key`（缺密钥则设备 Token 不可恢复）；
  运行中备份用 `sqlite3 devmask-manager.db ".backup devmask-manager.db.bak"`，密钥单独复制。
- Windows 无 Unix 权限位，`devmask-manager.key` / `manager.token` 仅提示用户自行保护，文档注明。

### 4.2 配置文件 manager.toml

默认路径：`<data-dir>/manager.toml`；可用 `--config` / `DFM_CONFIG` 指定。文件不存在不报错。

    # Manager / Relay 配置（全部可省略）
    bind = "127.0.0.1:8765"
    static_dir = "/opt/devmask-manager/webui/dist"
    token = "admin123"
    apply_concurrency = 4
    sync_concurrency = 4
    request_timeout_ms = 10000
    write_timeout_ms = 15000

    [manager]
    # 也可写成 [manager] token = "..."；与顶层 token 等价，[manager] 优先
    token = "admin123"

- `data_dir` 不接受配置文件自举（配置文件本身在数据目录内）；只认 CLI / 环境变量。
- 未知键：启动时 warn，不阻断（便于回滚旧配置）。
- 数值范围：`apply_concurrency` / `sync_concurrency` ∈ 1..=16，越界 → 启动失败；
  `request_timeout_ms` / `write_timeout_ms` 为 u64，当前实现不做上下限校验，
  建议保持默认（读 10 s / 写 15 s）；设为 0 会让所有设备请求立即超时。

### 4.3 参数与环境变量优先级

| 配置项 | CLI | 环境变量 | manager.toml | 默认 |
| --- | --- | --- | --- | --- |
| 监听 | `--bind` | `DFM_BIND` | `bind` | `127.0.0.1:8765` |
| 数据目录 | `--data-dir` | `DFM_DATA_DIR` | — | 平台默认（§4.1） |
| 静态目录 | `--static-dir` | `DFM_STATIC_DIR` | `static_dir` | 见 §11.1 |
| Token | `--token` | `DFM_TOKEN` | `[manager].token` / `token` | `admin123` |
| Token 文件 | `--token-file` | `DFM_TOKEN_FILE` | — | `<data-dir>/manager.token` |
| 配置文件 | `--config` | `DFM_CONFIG` | — | `<data-dir>/manager.toml` |
| 密钥文件 | `--key-file` | `DFM_KEY_FILE` | — | `<data-dir>/devmask-manager.key` |
| 日志级别 | `--log-level` | `DFM_LOG` / `RUST_LOG` | — | `info` |
| 下发并发 | — | `DFM_APPLY_CONCURRENCY` | `apply_concurrency` | 4 |
| 同步并发 | — | `DFM_SYNC_CONCURRENCY` | `sync_concurrency` | 4 |

Token 的**完整**优先级（高 → 低）：`--token` → `DFM_TOKEN` → `manager.token` 文件 → `manager.toml` → 默认 `admin123`。
`--token-file` 只改文件位置，不改变优先级。

其它 CLI：`--help`、`--version`（输出 `devmask_manager 0.1.0` 后退出）。

---

## 5. SQLite 数据模型

### 5.1 连接策略

- 单连接：`Db { conn: Arc<Mutex<Connection>> }`；所有数据库操作在 `tokio::task::spawn_blocking` 中执行，
  避免阻塞 async 执行器。
- 打开参数：`journal_mode = WAL`、`foreign_keys = ON`、`busy_timeout = 5000`、`synchronous = NORMAL`。
- 写事务用 `conn.unchecked_transaction()`（连接被 Mutex 独占，无重入）；读走普通查询。
- 规模假设：≤ 200 台设备、≤ 500 个应用、job 保留 200 条；单写连接足够。
  若未来做多用户，再引入 `r2d2_sqlite` 连接池（非目标）。
- 每次启动执行迁移（§5.3）；迁移与崩溃恢复在监听端口之前完成。

### 5.2 DDL（schema version 1 冻结 + v2 增量）

    PRAGMA journal_mode = WAL;
    PRAGMA foreign_keys = ON;
    PRAGMA busy_timeout = 5000;
    PRAGMA synchronous = NORMAL;

    CREATE TABLE IF NOT EXISTS schema_migrations (
      version    INTEGER PRIMARY KEY,
      applied_at TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS devices (
      id             TEXT PRIMARY KEY,
      name           TEXT NOT NULL,
      base_url       TEXT NOT NULL UNIQUE,
      token_cipher   TEXT NOT NULL,
      tags           TEXT NOT NULL DEFAULT '[]',   -- JSON string[]
      online         INTEGER NOT NULL DEFAULT 0,    -- 0/1
      model          TEXT,
      brand          TEXT,
      android        TEXT,
      sdk            INTEGER,
      fingerprint    TEXT,
      root_manager   TEXT,
      module_version TEXT,
      server_version TEXT,
      last_seen      TEXT,                          -- RFC 3339 UTC
      last_synced_at TEXT,
      latency_ms     INTEGER,
      last_error     TEXT,                          -- JSON {code,message} 或 NULL
      created_at     TEXT NOT NULL,
      updated_at     TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_devices_online  ON devices(online);
    CREATE INDEX IF NOT EXISTS idx_devices_created ON devices(created_at);

    CREATE TABLE IF NOT EXISTS device_overviews (
      device_id  TEXT PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
      revision   TEXT,                              -- 设备 config.revision
      fetched_at TEXT NOT NULL,
      payload    TEXT NOT NULL                      -- 设备 /overview 原文 JSON
    );

    CREATE TABLE IF NOT EXISTS jobs (
      id              TEXT PRIMARY KEY,
      kind            TEXT NOT NULL CHECK (kind IN ('bulk','assignments')),
      status          TEXT NOT NULL CHECK (status IN ('queued','running','succeeded','partial','failed')),
      app             TEXT,
      template        TEXT,
      restart         INTEGER NOT NULL DEFAULT 0,
      total           INTEGER NOT NULL,
      succeeded       INTEGER NOT NULL DEFAULT 0,
      skipped         INTEGER NOT NULL DEFAULT 0,
      failed          INTEGER NOT NULL DEFAULT 0,
      request_id      TEXT NOT NULL,
      idempotency_key TEXT UNIQUE,
      actor           TEXT NOT NULL DEFAULT 'manager-token',
      error           TEXT,                          -- JSON {code,message}；崩溃恢复写 interrupted
      created_at      TEXT NOT NULL,
      started_at      TEXT,
      finished_at     TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_jobs_created ON jobs(created_at DESC);
    CREATE INDEX IF NOT EXISTS idx_jobs_status  ON jobs(status);

    CREATE TABLE IF NOT EXISTS job_results (
      job_id      TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
      ordinal     INTEGER NOT NULL,                 -- assignment 顺序
      device_id   TEXT NOT NULL,
      app         TEXT NOT NULL,
      status      TEXT NOT NULL CHECK (status IN ('pending','ok','skipped','failed')),
      code        TEXT,
      http_status INTEGER,
      revision    TEXT,
      attempts    INTEGER NOT NULL DEFAULT 1,
      duration_ms INTEGER,
      restart     TEXT,                             -- JSON RestartResult 或 NULL
      steps       TEXT NOT NULL DEFAULT '[]',       -- JSON JobStep[]
      warnings    TEXT NOT NULL DEFAULT '[]',       -- JSON Warning[]
      error       TEXT,                             -- JSON ApiErrorPayload 或 NULL
      PRIMARY KEY (job_id, device_id, app)
    );
    CREATE INDEX IF NOT EXISTS idx_job_results_job ON job_results(job_id, ordinal);

    CREATE TABLE IF NOT EXISTS audit (
      id          INTEGER PRIMARY KEY AUTOINCREMENT,
      ts          TEXT NOT NULL,
      actor       TEXT,
      action      TEXT NOT NULL,
      target_type TEXT,
      target_id   TEXT,
      request_id  TEXT,
      status      TEXT NOT NULL DEFAULT 'ok',
      detail      TEXT                              -- JSON object 或 NULL
    );
    CREATE INDEX IF NOT EXISTS idx_audit_action ON audit(action);
    CREATE INDEX IF NOT EXISTS idx_audit_ts     ON audit(id DESC);

与计划书 §5 草图的差异（均为冻结结果）：

| 计划书 | 本实现 | 原因 |
| --- | --- | --- |
| `jobs(id, status, total, succeeded, skipped, failed, created_at, finished_at)` | 增加 `kind/app/template/restart/request_id/idempotency_key/actor/error/started_at` | 需要幂等键、请求关联与崩溃标记；assignment 明细在 `job_results` |
| `job_results` 基础列 | 增加 `ordinal/attempts/revision/steps/restart/warnings/error` | API 要返回逐设备 steps 与重试次数 |
| `audit.target TEXT` | `target_type + target_id` | 前端要按类型渲染，拆列避免每次解析 |
| 无 overview 缓存 | `device_overviews` | 矩阵 / 总览必须离线可用，不能每次遍历设备 |

### 5.2.1 v2 增量 DDL（egress）

v1 的 DDL 一行不改；v2 只追加三张表，并扩容 `jobs.kind` 的 CHECK：

    -- v2-1 代理池（凭据只存密文；AAD 见 §6.2）
    CREATE TABLE IF NOT EXISTS egress_proxies (
      id               TEXT PRIMARY KEY,                    -- egp_<32 hex>
      name             TEXT NOT NULL UNIQUE,                 -- ^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$
      kind             TEXT NOT NULL CHECK (kind IN ('http','socks5')),  -- 线上字段叫 type
      host             TEXT NOT NULL,
      port             INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
      username_cipher  TEXT,
      password_cipher  TEXT,
      region           TEXT,
      tags             TEXT NOT NULL DEFAULT '[]',            -- JSON string[]
      enabled          INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
      created_at       TEXT NOT NULL,
      updated_at       TEXT NOT NULL,
      CHECK (username_cipher IS NULL OR username_cipher LIKE 'v1:%'),
      CHECK (password_cipher IS NULL OR password_cipher LIKE 'v1:%')
    );
    CREATE INDEX IF NOT EXISTS idx_egress_proxies_name    ON egress_proxies(name);
    CREATE INDEX IF NOT EXISTS idx_egress_proxies_enabled ON egress_proxies(enabled);

    -- v2-2 绑定（app='' 即整机；mode=direct 时 proxy_id 必须为 NULL）
    CREATE TABLE IF NOT EXISTS device_egress (
      device_id  TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
      app        TEXT NOT NULL DEFAULT '',
      mode       TEXT NOT NULL CHECK (mode IN ('proxy','direct')),
      proxy_id   TEXT REFERENCES egress_proxies(id) ON DELETE RESTRICT,
      updated_at TEXT NOT NULL,
      PRIMARY KEY (device_id, app),
      CHECK ((mode = 'proxy'  AND proxy_id IS NOT NULL)
          OR (mode = 'direct' AND proxy_id IS NULL))
    );
    CREATE INDEX IF NOT EXISTS idx_device_egress_proxy  ON device_egress(proxy_id);
    CREATE INDEX IF NOT EXISTS idx_device_egress_device ON device_egress(device_id);

    -- v2-3 探测结果（设备来源 + Manager 直连来源）
    CREATE TABLE IF NOT EXISTS egress_checks (
      id         INTEGER PRIMARY KEY AUTOINCREMENT,
      source     TEXT NOT NULL CHECK (source IN ('device','manager')),
      device_id  TEXT REFERENCES devices(id) ON DELETE CASCADE,   -- manager 直连探测为 NULL
      app        TEXT NOT NULL DEFAULT '',
      scope      TEXT NOT NULL DEFAULT 'device' CHECK (scope IN ('device','app')),
      proxy_id   TEXT REFERENCES egress_proxies(id) ON DELETE SET NULL,
      exit_ip    TEXT,
      latency_ms INTEGER,
      ok         INTEGER NOT NULL CHECK (ok IN (0,1)),
      error_code TEXT,
      checked_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_egress_checks_device  ON egress_checks(device_id, checked_at DESC);
    CREATE INDEX IF NOT EXISTS idx_egress_checks_proxy   ON egress_checks(proxy_id, checked_at DESC);
    CREATE INDEX IF NOT EXISTS idx_egress_checks_checked ON egress_checks(checked_at DESC);

`jobs.kind` 扩容必须**重建表**（SQLite 不能 `ALTER CHECK`），见 §5.3 的步骤顺序。列集合与 v1 完全一致，只把 CHECK 放宽为 `IN ('bulk','assignments','egress')`。

与 [API.md §5.15](API.md#515-egress-三张表schema-v2) 一致；与计划书 §8 的差异只有两处：追加 `user_id 空值 + source/scope` 两列（Manager 直连探测没有设备，两条来源必须可区分）与 `ON DELETE` 语义（proxy 被引用时 RESTRICT、历史 check 用 SET NULL 保留）。

### 5.3 迁移

- 迁移在启动时串行执行：读 `schema_migrations` 最大 `version`，逐版本在事务内执行并写入一行。
- v1 即 §5.2 全量 DDL；后续版本只追加 `CREATE/ALTER`，不修改历史迁移。
- 迁移失败 → 启动失败、不接受请求；错误是 `DbError::Migration`，message 里带
  `op=migrate, current=<from>, target=<to>` 与底层原因（v2 的 DDL + jobs 重建 + 版本行在同一事务内）。
- 不提供降级迁移；回滚 = 用备份的 `devmask-manager.db` + `devmask-manager.key` 覆盖。
- **v2 的作业顺序（冻结）**：

      1) PRAGMA foreign_keys = OFF;         -- 连接级；必须先关
      2) BEGIN;
      3) 执行 §5.2.1 三张表 + jobs 重建配方（建 jobs_v2 → 搬行 → DROP jobs → RENAME）
      4) 重建 idx_jobs_created / idx_jobs_status
      5) COMMIT;
      6) PRAGMA foreign_keys = ON;
      7) PRAGMA foreign_key_check;           -- 必须为空集，否则迁移失败 → 启动失败

- 第 1 步是**硬要求**：`job_results.job_id REFERENCES jobs(id) ON DELETE CASCADE`，FK 开启时 `DROP TABLE jobs`
  会静默删掉历史结果（v1 库升级时会丢数据）；单元测试
  `db::tests::upgrades_v1_database_without_data_loss`（[../src/db.rs](../src/db.rs)）覆盖了
  「v1 库升级不丢 devices/jobs/job_results/audit + 迁移后 foreign_keys 恢复为 ON + 重复迁移不追加版本行」。
- 迁移可重复执行：`CREATE TABLE IF NOT EXISTS` + 一次性搬行；已到 v2 的库再跑一次是空操作。
- v2 迁移完成后 `GET /healthz` 的 `db.schemaVersion` 变为 `2`。

### 5.4 JSON 列约定

| 列 | 形状 |
| --- | --- |
| `devices.tags` | `string[]` |
| `devices.last_error` | `{code, message}` |
| `jobs.error` | `{code, message}` |
| `job_results.steps` | [API.md §5.7 JobStep](API.md#57-jobjobresultjobstepjobsummary) |
| `job_results.restart` | `RestartResult` |
| `job_results.warnings` | `Warning[]` |
| `job_results.error` | `{code, message, details, retryable}` |
| `audit.detail` | 任意 JSON 对象；禁止 Token / 密文 |

解析失败按「数据损坏」处理：读路径返回 `internal_error`（`details.op="decode"`），
写路径拒绝写入；不得把坏 JSON 透给客户端。

---

## 6. 设备 Token 加密

### 6.1 密钥文件

- 路径：`<data-dir>/devmask-manager.key`（可用 `--key-file` / `DFM_KEY_FILE` 覆盖）。
- 内容：**恰好 32 字节原始二进制**（AES-256 密钥），不是 hex / base64。
- 首次启动：`OsRng` 生成 32 字节，`OpenOptions::new().create_new(true).mode(0o600)` 写入并 `fsync`；
  已存在则读取，长度不是 32 → 启动失败（明确提示「密钥文件损坏，请恢复备份」）。
- Unix 上检测权限：若 `mode & 0o077 != 0` → 记 warn 并 `chmod 0600`；Windows 只记 warn。
- 日志只打印相对文件名，不打印内容。

### 6.2 加密格式

    token_cipher = "v1:" + BASE64_STANDARD( nonce[12] || ciphertext || tag[16] )

| 项 | 值 |
| --- | --- |
| 算法 | AES-256-GCM |
| 密钥 | `devmask-manager.key` 的 32 字节 |
| nonce | 每次加密随机 12 字节（`OsRng`），**绝不重用** |
| AAD（设备 Token） | 设备 id 的 UTF-8 字节（绑定行，防止密文搬迁） |
| AAD（egress 凭据） | `"egress_proxy:" ‖ proxy_id ‖ ":" ‖ field`，`field ∈ {username, password}`：在绑定 proxy id 之上再绑字段名，防止两列密文互换后仍能解密 |
| 版本前缀 | `v1:`；未知前缀 → 解密失败，按 `internal_error` 处理 |
| 编码 | 标准 Base64（带 `=` 填充） |

### 6.3 生命周期

1. 添加设备：校验 Token → 立即加密 → 只把 `token_cipher` 写库；明文在函数结束时 `Zeroizing` 清零。
2. 调用设备：从库读密文 → 解密得到 `Zeroizing<String>` → 构造 `Authorization` 头 → 请求结束即释放。
3. 修改 Token（`PATCH /devices/{id}`）：先用新 Token 探测，成功后才覆盖密文；
   探测失败保持旧值（事务回滚）。
4. 移除设备：删除行与密文；设备端 Token 不变（文档明确「只从管理端移除」）。

### 6.4 禁止清单（验收项）

- 不在任何响应 DTO 中序列化 `token` / `token_cipher`（DTO 结构体**没有**该字段）。
- 不在 tracing 中记录 `Authorization`、Token、密文；`ApiError` 的 `Debug` 手写脱敏。
- 不把 Token 写入 `audit.detail`；`PATCH` 审计只记录字段名。
- 异常路径（reqwest 错误、设备 401）不得把请求头拼进 message。
- WebUI 只在添加设备表单里**输入** Token，从不读取展示；列表 / 详情无 Token 字段。
- egress 代理凭据沿用同一把 `devmask-manager.key`：DTO **没有** `username` / `password` 字段（唯一信号 `hasCredentials`），
  审计 `detail` 只允许 `{proxyId, name, deviceId, app, changed}`，日志不打印解密后的值，错误 message 不回显凭据。
- 设备内认证中继（relay，设备端 v0.0.1）：`DeviceSummary.egress.relay = {enabled, active, listeners, firewallApplied}`（`active = enabled && 至少一个 listener listening`，来源设备 `GET /egress` 的 `relay`；设备离线 / 旧设备为 `null`）；`/devices/{id}/egress` 的 `deviceState` **原样透传** `relay` 与 `proxies[].localPort` / `relayActive`，不重算。
- Manager 侧探测自 v0.0.1 开启 reqwest 的 `socks` feature：`socks5` 代理用 `socks5h://`（DNS 交给代理解析）+ RFC 1929 真连，不再返回 `unsupported_proxy_type`。

密钥丢失 = 所有设备 Token 不可解密：只能删除并重新添加设备（文档明示，不做「找回」）。

---

## 7. 鉴权与安全边界

### 7.1 校验

- 提取 `Authorization`；必须是 `Bearer <token>`（大小写不敏感匹配前缀，值区分大小写）。
- 用 `subtle::ConstantTimeEq` 比较当前 Token；长度不同也走一次比较，避免长度侧信道。
- 失败 → `401 unauthorized`，`details = null`，审计 `auth.failed`（`detail = {path, reason}`，不含 Token）。
- 401 响应带 `WWW-Authenticate: Bearer`。

### 7.2 Token 轮换

    POST /token/rotate
      ├── body 畸形 / 非 JSON → 4xx 信封，且不轮换
      ├── 来源是 Cli/Env → 409 invalid_state（不可持久化）
      ├── 生成或校验新 Token（16–128 字符；生成 dfm_ + 32 base64url）
      ├── 先写 manager.token（临时文件 + rename，0600），失败 → tokenPersisted=false 但仍热替换
      ├── 更新 RwLock<TokenState>
      └── 审计 token.rotate（不含 Token）

### 7.3 启动安全闸门

- `bind` 的主机解析为 loopback（`127.0.0.0/8`、`::1`、`localhost`）→ 允许默认 Token。
- 非 loopback 且 Token 为默认 `admin123` 或长度 < 8 → **拒绝启动**，stderr 打印生成方法：

      error: refusing to bind non-loopback 0.0.0.0:8765 with default token
      hint: set a strong token via --token, DFM_TOKEN, or manager.token

### 7.4 其它边界

- 无 Cookie / Session → 无 CSRF 面；不启用 CORS（WebUI 与 API 同源；Vite 开发用代理）。
- 请求体上限 1 MiB（代理透传同样限制）；响应压缩只作用于 JSON / 静态文本。
- `baseUrl` 只允许 `http` / `https`；禁止 `file:` / `gopher:` 等 scheme；
  允许 loopback 与私网（`adb forward` / 内网设备是正常用法），这是**明确的运维模型**，不是 SSRF 漏洞：
  只有持有 Manager Token 的人能注册设备。
- 代理只访问已注册设备的 `baseUrl`，路径固定 `/api/v1/` 前缀，不拼接用户提供的绝对 URL；
  路径段拒绝 `..`/`.`/空段/反斜杠，请求体上限 1 MiB、设备响应上限 8 MiB。
- `POST /token/rotate` 原子写入 `<data-dir>/manager.token`（0600），内存热替换；CLI/env 固定的 Token 返回 409。
  `manager.token` 存在但不可读时启动失败（fail closed），不会静默回退默认 Token。
- 静态文件服务禁止目录穿越（路径组件拒绝 `..` 与绝对路径，并对 canonicalize 结果做前缀校验）；不提供上传端点。
- 不执行 shell；代理 / 下发全部通过 HTTP。
## 8. 设备客户端（device_client.rs）

### 8.1 baseUrl 归一化

入库前必须归一化（`url::Url`），规则按顺序执行：

| 步骤 | 规则 | 失败 |
| --- | --- | --- |
| 1 | `Url::parse` 成功 | `invalid_base_url` |
| 2 | scheme ∈ {`http`, `https`} | `invalid_base_url` |
| 3 | `host_str()` 非空 | `invalid_base_url` |
| 4 | `username()` / `password()` 为空（拒绝 userinfo） | `invalid_base_url` |
| 5 | `query()` / `fragment()` 为空 | `invalid_base_url` |
| 6 | host 小写；默认端口去掉（http:80 / https:443） | — |
| 7 | `path()` 去掉尾部 `/`；`/` 归一为 `""` | — |
| 8 | 序列化后作为 `base_url` 入库；查询时拼接 `/api/v1/<rel>` | — |

允许的路径前缀示例：`http://10.0.0.5:8686/df` → 请求 `http://10.0.0.5:8686/df/api/v1/status`。
不允许的示例：`ftp://x`、`http://u:p@host:8686`、`http://host:8686/?a=1`、`file:///etc/passwd`、`http://`。

### 8.2 请求构造

- 单例 `reqwest::Client`：`connect_timeout(3s)`、`pool_idle_timeout(90s)`、`user_agent("devmask-manager/0.1.0")`、
  `redirect(Policy::none())`（避免设备误配跳转把 Token 带到别的主机）。
- 每次请求设置 `timeout`：读 10 s、写 15 s、restart 10 s（来自配置，见 §4.2）。
- 请求头固定：`Authorization: Bearer <device token>`、`Accept: application/json`、
  `X-Request-Id: <manager request id>`、有体时 `Content-Type: application/json`。
- 请求体只由 Relay 构造（模板 / 绑定 / restart），大小上限 1 MiB。
- 不跟随重定向、不使用代理环境变量（`Client::builder().no_proxy()`），避免企业代理意外截获设备 Token。

### 8.3 错误分类

    pub enum DeviceCallError {
        Offline { reason: OfflineReason },            // connect | dns | timeout
        Http { status: u16, code: String, message: String, details: Value },
        Decode { detail: String },
        TooLarge { limit: usize },
    }

| 内部错误 | API 错误码 | HTTP | retryable |
| --- | --- | --- | --- |
| `Offline{connect\|dns\|timeout}` | `device_offline` | 502 | true |
| `Http{status>=500}` 或未知 code | `device_error` | 502 | true |
| `Http{status=401}` | `forbidden_scope`（result）/ `device_error`（代理） | 502 / — | false |
| `Decode` | `device_error` | 502 | true |
| `TooLarge` | `device_error`（`reason="response_too_large"`） | 502 | true |

设备返回体优先解析统一错误信封（`error.code/message/details`）；解析失败时 `code="device_error"`，
`message` 只包含状态码与路径，不包含响应体（可能含设备配置内容）。

### 8.4 并发与串行

- 全局 `sync_sem`（默认 4）限制 `/devices/sync` 的并发设备数。
- 全局 `apply_sem`（默认 4）限制同时执行的 assignment 数。
- 每台设备一把 `tokio::sync::Mutex`（`device_locks`）：同一设备的下发 / 同步串行，
  避免两个 job 同时改同一台设备的 revision。
- 设备删除时移除对应锁；锁表用 `std::sync::Mutex<HashMap<..>>`，只在取用时短暂持有，
  不在 async 上下文跨 await 持有。
- `job_sem` 限制同时运行的 job 数为 2；其它 job 保持 `queued`。

### 8.5 探测（添加 / 修改 / 同步共用）

    async fn probe(client, base_url, token) -> Result<ProbeOutcome, ProbeError> {
        let t0 = Instant::now();
        let status = get("/api/v1/status")?;      // 10 s
        let overview = get("/api/v1/overview")?;  // 10 s
        let latency_ms = t0.elapsed().as_millis() as u64;
        Ok(ProbeOutcome { status, overview, latency_ms })
    }

| 失败 | ProbeError.reason | HTTP |
| --- | --- | --- |
| 连接 / DNS / 超时 | `connect` / `dns` / `timeout` | 422 `device_probe_failed` |
| 设备 401 | `unauthorized` | 422 |
| 设备 403 | `forbidden_scope` | 422 |
| 其它非 2xx | `upstream_status`（`upstream={status,code}`） | 422 |
| 2xx 但缺 `device` 或 JSON 坏 | `invalid_response` | 422 |

探测成功才写库（§4.3 的「不留脏数据」）：设备行与 overview 缓存同一事务写入。

---

## 9. 聚合与缓存

### 9.1 缓存写入点

| 触发 | 写入内容 |
| --- | --- |
| `POST /devices` 探测成功 | 设备行 + `device_overviews`（overview 原文）+ 审计 |
| `PATCH /devices/{id}` 探测成功 | 设备行（含新 `base_url` / 密文）+ overview 缓存 |
| `POST /devices/{id}/sync` | 设备行 + overview 缓存 |
| `POST /devices/sync` | 逐台同上 |
| `GET /devices/{id}/overview`（默认实时） | overview 缓存 |
| job 完成 | 对结果为 `ok` 的设备做一次 best-effort overview 刷新（失败只记 warn，不改 job 结果） |

### 9.2 过期与警告

- `TTL = 300 s`：`fetched_at` 超过 300 s → 视图 `stale=true`，并附 `stale_cache` 警告（含 `ageSec`）。
- 设备 `online=false` → 视图同样 `stale=true`，但保留缓存（矩阵仍展示最后一次已知配置）。
- 设备没有缓存行 → `device_cache_missing` 警告，矩阵对应列全部为 `null`。
- 过期**不**触发自动刷新：本增量没有后台同步（非目标），由 UI 的「同步」按钮或 job 完成刷新。

### 9.3 矩阵计算（纯函数）

    pub fn build_matrix(devices: &[DeviceRow], overviews: &HashMap<String, DeviceOverview>, now: DateTime<Utc>) -> AppsMatrixResponse

算法（与 [API.md §4.12](API.md#412-get-apps) 规则 1–10 一一对应）：

1. 建立 `package -> Vec<(device_id, Cell)`。
2. 遍历设备（按 `/devices` 顺序），再遍历该设备 overview 的 `apps[]`：
   `cell.model = resolved.model`、`cell.template = source.template`、`cell.source = source.type`、
   `cell.installed = installed`、`cell.warnings = warnings`。
3. 行按 `package` 升序；列顺序固定为设备顺序。
4. `configured` / `models` / `inconsistent` / `cell.drift` 按 API 规则计算。
5. 行警告：所有单元警告去重 + 漂移警告。
6. 顶层警告：缓存缺失 / 过期 / 无设备。

该函数不访问数据库与网络，输入输出都是纯数据，单元测试覆盖：空设备、全一致、缺配置、两种机型、
离线设备、无缓存设备。

### 9.4 聚合 revision

    let mut pairs: Vec<String> = overviews
        .iter()
        .map(|(id, ov)| format!("{id}:{}", ov.config.revision))
        .collect();
pairs.sort();
let mut hasher = Sha256::new();
for pair in &pairs { hasher.update(pair.as_bytes()); hasher.update(b"\n"); }
let revision = if pairs.is_empty() { None } else { Some(hex::encode? 使用 sha2 输出格式化) };

- 只计入有缓存的设备；任何缓存刷新都会改变聚合 revision。
- 输出 64 位小写 hex；无缓存为 `null`。
- 与设备 revision 不同层级，不能作为 `If-Match`（API.md §5.10）。

### 9.5 模板与总览聚合

- `/templates`：遍历缓存 overview 的 `templates[]`，按 `name` 合并；
  同名配置不一致 → 保留首个，附 `template_conflict`（`details={name, devices, values}`）。
  `fieldsSet` = `config` 的键（排除 `packages` 时也计入，按字典序）。
- `/overview`：设备统计取 `devices` 表；应用 / 漂移复用 §9.3 的结果（不重复计算全量矩阵，直接构建
  `fleet` 计数 + 前 20 行漂移）。
- 性能预算：200 台 × 500 应用 = 100k 单元，单次构建 < 20 ms；超过时日志 warn（不阻断）。

---

## 10. 下发引擎与 job 生命周期

### 10.1 入队

    POST /apply
      ├── 解析并校验（API.md §4.13），归一化成 Vec<Assignment>
      ├── Idempotency-Key 存在 → 按 key 查 jobs（不设 TTL）→ 命中则返回旧 job（Idempotent-Replay）
      ├── 事务：INSERT jobs(status=queued, total, request_id, idempotency_key, ...) → commit
      ├── spawn(run_job(state, job_id))
      └── 202 {jobId, status:"queued", total, queued:total, pollUrl}

- 入队事务内预创建 `job_results` 行（`status='pending'`）；每台设备完成时 `UPDATE job_results SET status=…, steps=…, ...`
  并同时 `UPDATE jobs SET succeeded=…+1 / skipped / failed`，两者同一事务。
- `queued` 字段 = `total - (succeeded + skipped + failed)`，由计数派生，不单独存列。

### 10.2 Runner

    async fn run_job(state, job_id) {
        let _permit = state.job_sem.acquire().await;          // 最多 2 个 job 并行
        mark_running(job_id);                                  // status=running, started_at
        let assignments = load_pending_assignments(job);         // job_results 中的 pending 行
        let mut tasks = JoinSet::new();
        for (ordinal, a) in assignments.enumerate() {
            // apply_sem 在任务内部获取，避免占用排队槽位
            tasks.spawn(apply_assignment(state.clone(), job_id.clone(), ordinal, a));
        }
        while tasks.join_next().await.is_some() {}
        finalize(job_id);                                      // 计算终态 + finished_at + 审计 job.finished
        refresh_ok_devices(job_id).await;                      // best-effort overview 刷新
    }

- **assignment 快照**：入队事务内为每个 assignment 在 `job_results` 预创建一行（`status='pending'`、
  `ordinal` 为请求顺序），用于进度持久化与崩溃追溯。runner 使用入队时构造的内存 plan 执行，
  与 pending 行的 `ordinal/device/app` 一一对齐；每行完成时在同一事务里更新为 `ok/skipped/failed` 并累加计数。
  进程重启时未完成的 job 会被标记为 `failed`（不做断点续跑），因此无需持久化完整模板计划。
  API 只返回 `status∈{ok,skipped,failed}` 的行，`pending` 是内部状态（DDL 的 CHECK 已包含它）。

### 10.3 单 assignment 流水线

    async fn apply_assignment(state, job_id, ordinal, a) {
        let t0 = Instant::now();
        let device = load_device(a.device_id)?;               // 已被删除 → failed device_not_found
        let _device_lock = state.device_lock(&device.id).lock().await;
        let token = decrypt(&device.token_cipher)?;           // Zeroizing<String>

        // step 1: revision
        let rev = match call(GET /config/revision).await {
            Ok(r) => { record("revision", ok, 200); r.revision }
            Err(DeviceCallError::Offline { reason }) => return skip(device_offline, reason),
            Err(e) => return fail(map(e)),
        };

        // step 2: template
        match a.template {
            Name(name) => {
                match call(GET /templates/{name}).await {
                    Ok(_) => record("template_get", ok),
                    Err(Http 404) => { record("template_get", skipped, template_not_found);
                                       return skip(template_not_found); }
                    Err(e) => return fail(map(e)),
                }
            }
            Object{name, config} => {
                match call(GET /templates/{name}).await {
                    Ok(existing) => {
                        record("template_get", ok);
                        if existing.template != config {
                            warnings.push(template_overwritten);
                            put_template(name, config).await?;    // 整体替换
                        }
                    }
                    Err(Http 404) => put_template(name, config).await?,  // 新建（If-Match: *）
                    Err(e) => return fail(map(e)),
                }
            }
        }

        // step 3: bind（最多 2 次尝试）
        let mut attempts = 0;
        loop {
            attempts += 1;
            match call(PUT /apps/{pkg}, If-Match: rev, {template:name, detachFromTemplates:true}).await {
                Ok(res) => { rev = res.revision; record("bind", ok); break; }
                Err(Http 412) if attempts == 1 => { rev = get_revision().await?; continue; }
                Err(Http 412) => return fail(config_revision_mismatch),
                Err(Http 404 resource=package) => return skip(package_not_installed),
                Err(Http 403) => return fail(forbidden_scope),
                Err(Http 422) => return fail(validation_failed),
                Err(e) => return fail(map(e)),
            }
        }

        // step 4: restart
        let restart = if job.restart {
            match call(POST /apps/{pkg}/restart, Idempotency-Key: job:device:pkg).await {
                Ok(_) => RestartResult::ok(),
                Err(Http 403) => { warnings.push(restart_failed); RestartResult::skipped("forbidden_scope") }
                Err(e)       => { warnings.push(restart_failed); RestartResult::failed(code) }
            }
        } else { None };

        // step 5: 落库（单事务：结果行 + 计数 + 设备 lastSeen/online）
        write_result(job_id, ordinal, status=ok, attempts, steps, restart, warnings, duration=t0.elapsed());
    }

- 所有步骤都记录 `JobStep`（step/status/httpStatus/code/durationMs），失败时同样写入已执行步骤。
- 步骤 2 的 `PUT /templates` 412 重试同样只重试一次；超过 → `config_revision_mismatch`。
- 步骤 4 的任何失败都不改变结果 `status`（只加警告），符合计划书「ops 不足只记 warning」。
- requestId 透传给设备，便于用设备端 `api-audit.jsonl` 对齐。

### 10.4 崩溃恢复与清理

| 时机 | 动作 |
| --- | --- |
| 启动（迁移后、监听前） | `UPDATE jobs SET status='failed', error='{"code":"interrupted",...}', finished_at=? WHERE status IN ('queued','running')`；pending 结果行保留 |
| 启动 | 清理 `jobs`：保留最近 200 条或最近 7 天（先到为准）；`job_results` 随外键级联 |
| 启动 | 清理 `audit`：保留最近 5000 条 |
| 启动 | 清理过期 `manager.token`？不清理（0600 文件长期有效） |
| 每次 `POST /apply` | 按 `idempotency_key` 唯一查询，不设 TTL；并发同 key 的 UNIQUE 冲突回查后重放 |
| 关闭 | 等待 job 句柄 ≤5 s；未完成 job 保持 `running`，下次启动按 interrupted 处理 |

### 10.5 幂等细节

- 键长度 ≤128；空白 / 换行 → 422。
- 唯一索引冲突（并发同键）= 捕获后按命中处理，返回已有 job。
- 响应头 `Idempotent-Replay: true` 只在命中时出现。
- 不比较请求体：同键不同体返回首个 job（API.md §2.11）。

### 10.6 可观测性字段

job 相关 tracing span 字段：`job_id`、`kind`、`total`、`status`、`duration_ms`；
assignment 日志（debug）：`job_id`、`device_id`、`app`、`step`、`http_status`、`attempts`。
成功不打印设备响应体；失败只打印 code 与 trace 需要的字段。

### 10.7 egress 下发语义（`kind="egress"`，Manager 侧）

设备端契约见 [../../docs/plans/egress-contract.md](../../docs/plans/egress-contract.md) §6；这里只冻结 Manager 的实现口径。

- **Manager 只拥有三样东西**：`default`（整机代理名）、`apps`（应用级覆盖）、`proxies`（被引用的代理定义 + 凭据）。
  `failOpen` / `probeUrl` / `probeTimeoutSeconds` / `exclusionList` 不在 Manager 的请求体里出现，
  设备侧保持现值：Manager 不下发、不比较、不重置。探测 URL 在 Manager 侧是常量
  `api::egress::DEFAULT_PROBE_URL`（可被 `POST /egress-proxies/{id}/probe` 的请求体 `url` 覆盖）。
- **零应用级绑定发 `apps: null`**：`[]` 与 `null` 在设备端都表示「没有应用级覆盖」，但契约只把
  `null` 定义为「清除所有 `[[apps]].egress`」，`[]` 依赖「整体替换为空数组」的实现细节。
  因此 Manager 在 `apps` 为空时发 `null`；非空时发完整数组（整体替换）。
- **总是 PUT，不做本地比较**：runner 每次下发都 `PUT /api/v1/egress`，用设备响应的 `changed`
  判定幂等短路（`changed:false` → 步骤 `egress_put` 记 `skipped` / `egress_unchanged`，结果行
  `code=egress_unchanged`、`status=ok`，job 因此不会变 `partial`）。这样配置漂移、凭据轮换、
  设备侧手改都能自然收敛，不需要额外的「已下发指纹」列。
- **凭据字段总是显式给出**：代理未设置用户名/密码时下发 `username:null` / `password:null`；
  省略在设备端是「不改该条已存凭据」，会把上一次的密码留在设备上。
- **assignments 形态先校验后落库**：`plan_egress_targets` 只做校验 / 归一化（返回合并后的行与分组），
  `persist_egress_bindings` 才写 `device_egress` + `egress.bind` 审计；`dryRun:true` 只走前者。
- **幂等键与 `POST /apply` 同规则**：1-128 字符、不含空白/控制字符（非法 422）；`insert` 失败时
  回查 `find_job_by_idempotency` 并重放，回查不到才 500。
- **egress 专属结果码**：`egress_unsupported`（旧设备无 `/egress`）、`egress_not_configured`
  （设备 409 `egress.not_configured`）、`decrypt_failed`（代理凭据解密失败，`details.op="decrypt"`，
  不是 `device_error`）、`device_token_invalid`（设备 Token 解密失败，message 提示 `devmask-manager.key` 可能已变更）。
- **探测结果写入失败不静默**：`egress_checks` 写失败时 job 结果追加 warning `egress_check_not_recorded`；
  `POST /egress-proxies/{id}/probe` 的响应体同样带该 warning。
- **egress_checks 清理**：`prune()` 每个 `(device_id, app)` 只保留最近 500 条（`checked_at DESC, id DESC`）。
- **Manager 侧探测自 v0.0.1 支持 `http` 与 `socks5`**：`socks5` 用 `socks5h://` + RFC 1929 真连（reqwest `socks` feature），失败同样落一条 `ok=0`
  记录（reqwest 未启用 socks feature）；probe URL 必须是 `http://`。

---

## 11. 静态托管

### 11.1 目录解析

顺序（先命中先用）：

1. `--static-dir <PATH>`
2. `DFM_STATIC_DIR`
3. `<exe 所在目录>/webui/dist`
4. `<cwd>/manager/webui/dist`
5. `<cwd>/webui/dist`
6. 都没有 → 「未构建 WebUI」提示页（仅 `GET /` 返回 200 文本）

命中目录要求包含 `index.html`；只有目录没有 index → 视为未构建，日志 warn。

### 11.2 路由与响应头

| 路径 | 处理 | Cache-Control |
| --- | --- | --- |
| `/assets/*` | 内置静态文件处理器（读文件 + MIME + 缓存头） | `public, max-age=31536000, immutable` |
| `/`、`/index.html` | `index.html` | `no-cache` |
| 其它存在的静态文件（`favicon.ico`、`logo.png`…） | 内置静态文件处理器 | `public, max-age=3600` |
| 未命中且非 API、且方法为 GET/HEAD | `index.html`（SPA fallback） | `no-cache` |
| 未命中且是 `/relay/api/*` | JSON 404（§API 4.18 之外的路由） | `no-store` |
| 非 GET/HEAD 的非 API 路径 | `405 method_not_allowed` | `no-store` |

统一附加：`X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer`、
`Content-Security-Policy: default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'`。
压缩只对 `text/*`、`application/json`、`application/javascript` 生效。

实现说明：静态文件由 `static_files.rs` 直接读取（`tokio::fs::read`）并设置 MIME / 缓存 / 安全头，
不启用 Range 请求；dist 产物都是小文件，足够且省掉额外中间件。

### 11.3 未构建时的行为

- `GET /` 返回 200 `text/html`：

      <h1>DevMask Manager</h1>
      <p>WebUI 尚未构建。运行：cd manager/webui && npm install && npm run build</p>

- 其它静态路径 404 `text/plain`；API 完全可用（便于后端联调与集成测试）。
- 集成测试不依赖 `dist`：测试用 `--static-dir` 指向临时目录 + 手写 index.html。

### 11.4 开发与构建

    cd manager/webui
    npm install
    npm run dev      # 5180，/relay 与 /healthz 代理到 8765
    npm run build    # 产物 dist，由 Manager 静态托管

- Vite 必须在 `vite.config.ts` 增加 §API 6 的代理配置；`npm run build` 前后端联调前必须通过。
- 发布形态（本增量）：二进制度制 + 同级 `webui/dist` 目录；单二进制内嵌（`include_dir`）留待后续。

---

## 12. 日志、审计与健康检查

### 12.1 tracing

- 初始化：`tracing_subscriber::fmt()` + `EnvFilter`；默认 `info`，`DFM_LOG=debug`
  （或 `--log-level`）打开详细日志。
- 启动日志：`version`、`bind`、`data_dir`、`token_source`、`address`；静态托管命中 / 未命中各一行；
  首次生成 `devmask-manager.key` 时一行 info。
- 请求日志由 `tower_http::trace::TraceLayer` 输出，默认级别 `debug`（`DFM_LOG=debug` 可见），
  字段含 method / path / status / latency。
- 使用默认 Token 时打印一行 warn（唯一一次出现默认值）：
  `正在使用默认 Manager Token admin123；建议用 --token / DFM_TOKEN / manager.token 轮换`。

### 12.2 脱敏

| 内容 | 处理 |
| --- | --- |
| `Authorization` 请求头 | 永不记录 |
| 设备 Token / `token_cipher` | 永不记录；`Zeroizing` 清零 |
| 请求 / 响应体 | 不记录（debug 也不记录） |
| 绝对路径 | API 响应与错误只出现相对文件名（`devmask-manager.db`）；启动 / 文件读写日志会打印完整路径 |
| 设备 401 响应体 | 只记录 `code`（如 `unauthorized`），不记录设备 message 与响应体 |

### 12.3 审计

| action | target.type / id | detail |
| --- | --- | --- |
| `auth.failed` | `token` / null | `{path, reason}`（reason = missing \| malformed \| mismatch） |
| `device.add` | `device` / id | `{baseUrl, online, model}` |
| `device.update` | `device` / id | `{fields: ["name","token"]}`（不含值） |
| `device.remove` | `device` / id | `{name, baseUrl}` |
| `device.sync` | `device` / id | `{online, latencyMs}` |
| `device.sync_all` | `device` / "all" | `{synced, failed}` |
| `apply.bulk` | `device` / `all`（devices=all）或 `multiple`（id 数组） | `{jobId, total, app, template}` |
| `apply.assignments` | `device` / `mixed` | `{jobId, total, assignments}` |
| `job.finished` | `job` / jobId | `{status, succeeded, skipped, failed, durationMs}` |
| `token.rotate` | `token` / null | `{source, tokenPersisted}` |
| `egress.proxy_create` | `egress_proxy` / id | `{proxyId, name, changed}` |
| `egress.proxy_update` | `egress_proxy` / id | `{proxyId, name, changed}` |
| `egress.proxy_delete` | `egress_proxy` / id | `{proxyId, name}` |
| `egress.proxy_probe` | `egress_proxy` / id | `{proxyId, name, changed:false}` |
| `egress.bind` | `device` / id | `{deviceId, changed, total}` |
| `egress.apply` | `job` / jobId | `{jobId, total, devices, changed}` |

- `apply.bulk` 的 `target_id` 为 `all` 或 `multiple`，`apply.assignments` 为 `mixed`（避免逗号拼接过长）；
  入队成功后立即写审计（失败只 warn，不影响 202）；assignment 明细不写审计（job_results 已保存）。
- 审计与业务写在同一事务（成功后写入）；`auth.failed` 单独写，失败只记 warn。
- 审计查询端点见 [API.md §4.16](API.md#416-get-audit)。

### 12.4 健康检查

`GET /relay/api/v1/healthz`（与 `/healthz` 同 handler）：

1. `SELECT 1` 与 `schema_migrations` 最大版本检查 → `db.status`。
2. 设备总数 / 在线数（各一条 COUNT）。
3. job queued / running 计数。
4. uptime = `now - started_at`。
5. 任何 DB 异常只降级 `status="degraded"`，HTTP 仍 200（避免探针重启风暴），日志 error 带 requestId。

---

## 13. 与设备端 `/api/v1` 的映射（实现级）

| Relay 操作 | 设备请求 | 关键头 | 超时 | 重试 | 结果 |
| --- | --- | --- | --- | --- | --- |
| 添加 / 修改设备 | `GET /api/v1/status` + `GET /api/v1/overview` | Authorization | 10 s ×2 | 无 | 失败 422 `device_probe_failed`，不落库 |
| `POST /devices/{id}/sync` | `GET /status` + `GET /overview` | Authorization | 10 s ×2 | 无 | 失败 → 200 + `device_offline` 警告 |
| `POST /devices/sync` | 同上（并发 4） | Authorization | 10 s ×2 | 无 | 逐台结果 |
| `GET /devices/{id}/overview`（实时） | `GET /overview` | Authorization | 10 s | 无 | 失败 502/504；`cached=1` 不调用 |
| `POST /apply` 步骤 1 | `GET /config/revision` | Authorization | 10 s | 无 | 失败 → skipped/failed |
| `POST /apply` 步骤 2 | `GET /templates/{name}`；404 时 `PUT /templates/{name}` | Authorization；PUT 加 `If-Match: *` | 10 s / 15 s | PUT 412 重试 1 次 | 见 API §7.2 |
| `POST /apply` 步骤 3 | `PUT /apps/{pkg}` | `If-Match: "<rev>"` + JSON | 15 s | 412 重试 1 次 | 见 API §7.2 |
| `POST /apply` 步骤 4 | `POST /apps/{pkg}/restart` | `Idempotency-Key: job:device:pkg` | 10 s | 无 | 失败只加警告 |
| `ANY /devices/{id}/proxy/api/v1/{*path}` | 同名路径 | 白名单透传 + 注入 Authorization | 读 10 s / 写 15 s | 无 | 见 API §4.18 |
| 聚合视图 | **不调用** | — | — | — | 只读 SQLite |

设备 Token 只出现在对设备的 `Authorization` 头中；代理不透传客户端 `Authorization`。

---

## 14. 测试策略

### 14.1 单元测试

| 模块 | 用例 |
| --- | --- |
| `crypto.rs` | 加密→解密往返；错误密钥失败；篡改密文失败；不同设备 id（AAD）不能互解；密钥文件 32 字节校验与 0600 |
| `device_client.rs` | baseUrl 归一化表（合法 / 非法 / 去尾斜杠 / 去默认端口 / 保留前缀）；URL 拼接；错误分类 |
| `ids.rs` | slug（英文 / 中文 / 符号 / 长度）；job id / request id 格式 |
| `apps.rs` | 矩阵算法：空设备、全一致、缺配置、两机型、离线、无缓存、cell.drift |
| `overview.rs` | 聚合 revision 稳定性与空集合；fleet 计数 |
| `jobs.rs` | 终态计算表（全 ok / 全 skipped / 混合 / 全 failed）；计数不变量 |
| `auth.rs` / `config.rs` | Bearer 解析、常量时间比较；Token 优先级（CLI > env > file > config > default）、非 loopback 默认 Token 拒绝、并发范围 |

### 14.2 集成测试

测试文件与覆盖（合计 72 个集成用例 + 56 个单元用例）：

| 文件 | 用例 | 覆盖 |
| --- | --- | --- |
| `tests/common/mod.rs` | — | 临时数据目录 + 进程内 Router（`build_router`）+ 请求助手 |
| `tests/common/mock_device.rs` | — | axum Mock 设备：status / overview / revision / templates / apps / restart / echo |
| `tests/manager_api.rs` | 17 | healthz、鉴权 401、请求 id、404/405、静态托管、no-store、目录/密钥/DB 权限、Token 加密 |
| `tests/devices_flow.rs` | 14 | 添加 / 探测失败 / 重复 / 详情 / 同步 / 离线 / 矩阵 / 过滤分页 / PATCH / 超时 / 未知字段 / 超大 overview |
| `tests/apply_flow.rs` | 14 | bulk / assignments、packages 保留、PUT 后重取 revision、412 重试、未安装、离线、模板缺失/覆盖、restart 403、幂等过期/并发重放、下发审计 |
| `tests/ops_endpoints.rs` | 8 | templates / overview / audit / token rotate / proxy / jobs 列表 |
| `tests/e2e_flow.rs` | 4 | 全端点单流程、离线 502 + 缓存 stale、设备 401 → 502、apply 参数校验 |
| `tests/review_reverify.rs` | 15 | t14 独立复验 R1-R9（packages、revision、幂等、no-store、rotate、响应上限、since、templates device）+ 低危；R6 含无 Content-Length 的 64 MiB chunked 提前中止 |

`tests/common/mod.rs` 提供：

    pub struct TestApp { router, state, token, data_dir, _temp: TempDir }
    pub async fn start_app(static_dir: Option<PathBuf>) -> TestApp
    pub async fn start_app_with_token(static_dir, TokenState) -> TestApp
    pub async fn send / send_json / send_json_with_headers / send_raw_json / send_raw_with_content_type(...)

`MockOptions`：`token`、`model`/`brand`/`android`/`sdk`/`fingerprint`、`templates`（GET 返回含 packages 的真实形状）、`apps`、
`force_first_412`、`missing_packages`、`restart_status`、`oversized_overview`、`fail_status`、`delay`。

| 用例 | 断言 |
| --- | --- |
| `GET /healthz` 无鉴权 | 200，`status=ok`，`apiVersion=v1`；根路径别名同样可用 |
| `GET /devices` 无 Token | 401 信封 + `WWW-Authenticate` |
| 添加设备 | 201；`online=true`；`model=Pixel 9`；响应不含 Token |
| 重复 baseUrl / id | 409 `device_exists` |
| 探测失败 | 422 `device_probe_failed`；`devices` 表为空；reason=upstream_status/unauthorized/connect |
| 列表过滤 / 分页 | `online` / `tag` / `q` / `limit` + `nextCursor` |
| 详情 | 含 `overview/templates/apps/cachedAt/stale` |
| PATCH 改名 / 换 Token | 200；换坏 Token → 422 且旧值保留 |
| DELETE | 204；再次 DELETE → 404 |
| sync 单台 / 全部 | 200；离线设备 `online=false` + 警告；`synced/failed` 计数 |
| apps 矩阵 | 列 = 设备；行 = 并集；`inconsistent` 与 `drift` 正确 |
| overview | fleet 计数、drift 行、recentJobs |
| templates | 并集 + `template_conflict` |
| apply bulk | 202；轮询终态 `succeeded`；`attempts=2`；结果 `steps` 含 revision/template_put/bind/restart |
| apply assignments | 不同设备不同模板；`kind=assignments` |
| 包未安装 | 结果 `skipped / package_not_installed`；job `partial` |
| 离线设备 | 结果 `skipped / device_offline`；job `partial`；实时 overview / proxy 为 502，缓存仍 200 且 `stale` |
| restart 403 | 结果仍 `ok`；`warnings` 含 `restart_failed` |
| 模板只给名字且不存在 | `skipped / template_not_found` |
| 幂等键重放 | 两次 202 同一 jobId；第二次带 `Idempotent-Replay: true` |
| 未知 API 路径 | 404 JSON `not_found`（非 HTML） |
| 静态托管 | 临时 dist 的 index.html 可访问；`/devices` 返回 index；`/assets/x.js` 有 immutable 头 |
| body > 1 MiB | 413 `body_too_large` |
| 代理 | 2xx / 设备 4xx 原样透传；设备 401 → 502 `device_error`；离线 → 502 `device_offline` |
| Token 轮换 | 响应一次新值；`manager.token` 0600；旧 401 / 新 200；Cli/Env 来源 409 |
| 审计 | 添加 / 全量同步 / 下发（`apply.bulk` / `apply.assignments`）/ `job.finished` / 轮换可查；`detail` 不含 Token |
| 崩溃恢复 | `db.rs` 单元测试：`recover_interrupted_jobs` 把 running 标记为 failed |

> 设备 401/403 → Relay 502 `device_error` 的 remap 由 `proxy.rs` 实现；
> `e2e_flow.rs` 通过运行中更换 Mock 设备 Token 覆盖 401 场景（403 由同一分支处理）。

### 14.3 命令

    cargo fmt --check
    cargo test -p devmask_manager
    cargo test -p devmask_manager --test e2e_flow
    cargo clippy -p devmask_manager --all-targets -- -D warnings

WebUI（frontend-dev / test-verifier）：

    cd manager/webui && npm install && npx vue-tsc --noEmit && npm run build

### 14.4 契约一致性检查

- [API.md §3](API.md#3-端点总表) 的 18 条路由由 `tests/e2e_flow.rs` 按顺序全体调用；
  新增 / 删除端点时必须同步该文件与 `docs/api-types.ts`。
- `manager/docs/api-types.ts` 用 `tsc --strict --noEmit` 校验；`manager/webui/src/api/types.ts`
  是它的只读副本，由 frontend-dev 同步（差异检查：`diff -u manager/docs/api-types.ts
  manager/webui/src/api/types.ts`）。

---

## 15. 构建、运行与部署

### 15.1 构建

    # 开发
    cargo build -p devmask_manager
    # 发布（工作区 profile：lto=fat、strip、panic=abort）
    cargo build -p devmask_manager --release
    # WebUI
    cd manager/webui && npm install && npm run build

产物：`target/release/devmask_manager`（单文件）+ `manager/webui/dist`。

### 15.2 运行

    # 默认：127.0.0.1:8765，数据目录按平台默认，Token=admin123
    ./devmask_manager

    # 显式指定
    ./devmask_manager \
      --bind 127.0.0.1:8765 \
      --data-dir ~/.local/share/devmask-manager \
      --static-dir manager/webui/dist \
      --token 'S3cure-manager-token'

    # 局域网访问（必须换 Token，否则拒绝启动）
    DFM_TOKEN='S3cure-manager-token' ./devmask_manager --bind 0.0.0.0:8765

完整步骤（WebUI 构建、数据目录、Token、Mock 设备、端到端冒烟、常见错误）见
[RUN.md](RUN.md)。

### 15.3 服务托管（示例）

systemd（Linux）：

    [Unit]
    Description=DevMask Manager
    After=network-online.target

    [Service]
    ExecStart=/opt/devmask-manager/devmask_manager --data-dir /var/lib/devmask-manager --static-dir /opt/devmask-manager/webui/dist
    Environment=DFM_TOKEN=__REPLACE__
    User=devmask
    NoNewPrivileges=true
    ProtectSystem=strict
    ReadWritePaths=/var/lib/devmask-manager
    Restart=on-failure

    [Install]
    WantedBy=multi-user.target

macOS launchd 与 Windows 服务只写 README 的等价说明（本增量不打包安装器）。

### 15.4 升级与备份

1. 停服 → 备份 `devmask-manager.db` + `devmask-manager.key`（+ `manager.token`）。
2. 替换二进制与 `webui/dist`。
3. 启动 → 自动迁移（只前进）→ 检查 `/healthz` 的 `db.schemaVersion`。
4. 回滚 = 恢复备份文件 + 旧二进制。

---

## 16. 写作用域（团队协作）

| 角色 | 允许修改 | 本增量任务 |
| --- | --- | --- |
| backend-dev | `manager/src`、`manager/Cargo.toml`、`manager/docs`、`manager/tests`、根 `Cargo.toml` 的 members | contract、backend-core、backend-devices、backend-apply、repair |
| frontend-dev | `manager/webui` | frontend-api |
| test-verifier | `manager/tests`、`manager/docs`、`manager/README.md` | test-docs、verify |
| reviewer | 只读 + 评审报告 | review |

- 根 `Cargo.toml` 只允许在 backend-core 任务里追加 `members` 条目（`"manager"`），不动其它段落。
- `manager/docs/API.md` 与 `ARCH.md` 由 backend-dev 在 contract 任务冻结；后续变更必须同时写
  [§18 变更记录](#18-变更记录)（API.md 为 §10）。

---

## 17. 已知缺口与后续

| 项 | 现状 | 后续 |
| --- | --- | --- |
| TLS / 公网 | 只绑 loopback 或内网；公网需前置反代 | 反向代理文档 / 可选 TLS |
| 限流 | 未实现（不返回 429） | 与设备端同步规划 |
| 多 Token / scope | 单 Token 全权限 | 需要时加 `tokens` 表与 scope 中间件 |
| 连接池 | 单写连接 + Mutex | 大舰队时换 r2d2 |
| 密钥轮换 | 不支持（丢失需重加设备） | 双密钥过渡方案 |
| 定时同步 / WebSocket | 无 | 后台任务 + SSE/WS |
| 模板库 | 从设备缓存聚合，不独立存储 | 独立模板仓库 |
| 应用名 / 图标 | 只显示包名 | 设备端或单独抓取 |
| 单二进制内嵌 WebUI | 未做（dist 目录） | `include_dir` 打包 |
| Windows 权限 | 只提示，不强制 | ACL 设置 |
| `request_timeout_ms` / `write_timeout_ms` 范围 | 未做上下限校验（§4.2） | 需要时补 1 s–120 s 校验 |
| assignment 总时限 | 合同 §2.10 目标 60 s，当前只按单请求超时（最坏可叠加） | 需要时为每个 assignment 套 `tokio::time::timeout` |

---

## 18. 变更记录

| 版本 | 日期 | 变更 |
| --- | --- | --- |
| v1.0 | 2026-11-24 | 冻结：进程模型、依赖、SQLite DDL、Token 加密、鉴权、静态托管、下发流水线、测试策略 |
| v1.1 | 2026-11-24 | 实现落地后的文档校正：超时数值不做范围校验（§4.2）；测试策略改为实际文件与用例（§14）；补充 RUN.md 入口（§15.2）；下发审计（`apply.bulk` / `apply.assignments`）已补齐（§12.3）；日志 / 脱敏按实现校正（§12）；登记 assignment 总时限缺口（§17） |

---

## 19. 许可

    SPDX-License-Identifier: GPL-3.0-or-later
    Copyright (c) 2025-2026 Seyud
    DevMask: https://github.com/uamind/DevMask
