# xcld —— XCL 账号服务

一个 Go 二进制，两件事：

- **HTTP**：玩家账号的注册、登录、续期、退出
- **TCP 桥接**：节点主入口（11010）。没有登录态，连接直接断，一个字节都不回

两者合在一个进程里不是图省事：桥接每条连接都要查「这个账号还在不在、被封了没有」，
分成两个服务就得反过来调 HTTP 或定时拉黑名单，既有延迟又会有「封了还能用一会儿」的窗口。

```
客户端 ──HTTPS──> Cloudflare ──> xcld :8080   （注册 / 登录 / 续期）
客户端 ──TCP────> xcld :11010  ──> easytier core :11011  （联机主链路）
```

Cloudflare 只做 HTTP 那一层（TLS、WAF、隐藏源 IP）。**TCP 它包不了**——免费版不代理
任意端口，Spectrum 是按流量计费的付费产品，所以数据面必须自己守着。

## 接口

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/v1/health` | 账号数、在线会话数、运行时长 |
| GET | `/v1/captcha` | 取人机验证方式与题目（Turnstile 时返回 siteKey） |
| POST | `/v1/auth/register` | 注册，成功直接返回登录令牌 |
| POST | `/v1/auth/login` | 登录 |
| POST | `/v1/auth/refresh` | 续期（`Authorization: Bearer`） |
| POST | `/v1/auth/logout` | 退出登录，立刻作废这条会话 |
| GET | `/v1/auth/me` | 当前账号 |

错误统一是 `{"code": "...", "message": "..."}`，前端按 `code` 分支，不要匹配文案：

`bad_request` `username_taken` `bad_credentials` `banned` `unauthorized`
`captcha_failed` `captcha_expired` `rate_limited` `server_error`

## 客户端要实现的协议

### 注册 / 登录

```jsonc
// POST /v1/auth/register
{
  "username": "xiaoxin",        // 3~20 位，仅字母/数字/_/-
  "password": "password123",    // 8~128 位
  "deviceName": "DESKTOP-XXX",  // 可选，只用于认出登录设备
  "captcha": { "provider": "pow", "challenge": "...", "nonce": "..." }
}

// 返回（注册和登录同构）
{ "token": "...", "expiresAt": 1796110667,
  "bridge": "bridge.example.com:11010",   // 见下：客户端该去连哪个地址
  "user": { "id": 1, "username": "xiaoxin", "createdAt": 1793518667, "lastLoginAt": 1793518667 } }
```

`bridge` 是**桥接地址**（服务端通过 `XCL_BRIDGE_ADVERTISE` 配置）。必须由服务端下发，
不能由客户端拿账号域名加个端口去猜 —— 账号接口走 Cloudflare，桥接走裸 TCP，
两者解析到的 IP 根本不是一个。

令牌有效期 30 天，剩 7 天时调 `/v1/auth/refresh` 静默续期。

### 人机验证（PoW 档）

`GET /v1/captcha` 会给 `challenge` 和 `difficulty`（当前 20）。客户端从 0 开始递增计数器：

```
nonce = 计数器按大端序编成 8 字节，再转小写 hex        // 0000000000000000, 0000000000000001, ...
命中条件 = sha256(challenge + "|" + nonce) 的前 difficulty 位全为 0
```

20 bit 大约一百万次 sha256，普通机器 0.3~1 秒。用多线程可以更快，算法本身不用变。
服务端的参考实现在 `internal/captcha/captcha.go` 的 `Solve`。

挑战 5 分钟过期、只能用一次；过期或重放会返回 `captcha_expired`，重新取题即可。

### 连节点

TCP 连上 `节点:11010`，**先发一帧**：

```
magic  "XCL1"   4 字节
长度   令牌文本长度（大端 u16）
载荷   令牌（base64url 文本，118 字节）
```

- 通过 → 服务端回 1 个字节 `0x00`，之后就是 easytier 的裸 TCP 隧道
- 不通过 → **服务端直接关连接，一个字节都不回**

客户端本地要做一个 TCP 代理：easytier 连本地代理 → 代理连桥接、发帧、等 `0x00`、
再开始双向转发。注意 `--use-smoltcp` **连不了 127.0.0.1**（`code 10049`），
所以本地代理必须绑「能出网那块网卡的 IP」，不能绑回环。

## 部署

```bash
./deploy/install.sh root@<服务器地址>
```

脚本会交叉编译（纯 Go，无 CGO）、装到 `/opt/xcld`、建系统账号 `xcld`、
首次安装时生成密钥写到 `/etc/xcld/xcld.env`、装 systemd 服务并启动。

**公钥要写进客户端**（`grep XCL_SIGN_PUBKEY /etc/xcld/xcld.env`）。

### 环境变量

| 变量 | 说明 |
|---|---|
| `XCL_SIGN_KEY` | 32 字节 seed 或 64 字节私钥的 hex，签发登录令牌 |
| `XCL_POW_SEED` | 32 字节 hex，签人机验证挑战 |
| `XCL_TURNSTILE_SECRET` | 配了就用 Cloudflare Turnstile，留空则用 PoW |
| `XCL_TURNSTILE_SITEKEY` | 上面那个对应的 sitekey，下发给客户端 |
| `XCL_BRIDGE_ADVERTISE` | 下发给客户端的桥接地址，形如 `bridge.example.com:11010`。**挂 CDN 时必填** |
| `XCL_MAX_PER_IP` | 同一 IP 的桥接并发连接数，默认 64 |
| `XCL_MAX_PER_DEVICE` | 同一账号的桥接并发连接数，默认 8 |
| `XCL_DEVICE_RATE` | 同一账号在建连窗口内的次数上限，默认 2000 |
| `XCL_IP_RATE` | 同一 IP 在建连窗口内的次数上限，默认 20000 |
| `XCL_RATE_WINDOW` | 建连速率窗口，默认 `10m` |
| `XCL_BAD_TOKEN_LIMIT` | 同一 IP 每分钟坏令牌次数，超过即临时拉黑，默认 20 |
| `XCL_BAD_TOKEN_BAN` | 拉黑时长，默认 `10m` |
| `XCL_TOKEN_CACHE` | 令牌校验结果缓存时长，`0` 表示每次都查库，默认 `60s` |
| `XCL_REGISTER_PER_DAY` | 全站每天注册上限（UTC+8 自然日，落库计数），默认 300 |
| `XCL_MAX_SESSIONS` | 单账号活跃登录态上限，默认 3 |
| `XCL_ACCEPT_RATE` | 每秒建连数上限，持续超过即熔断，默认 500 |
| `XCL_MEM_LIMIT_MB` | 进程内存上限，超过即停服，默认 1536 |
| `XCL_FD_RATIO` | 文件句柄使用比例上限，超过即停服，默认 0.8 |
| `XCL_LIMIT_DISABLE` | 设为 `1` 关掉频率限制与熔断（排障用，慎用）；**注册日上限不受影响** |

同名选项也能写成命令行参数（`--max-per-ip 128`），环境变量是它的默认值。

## 风控（被刷的时候会发生什么）

分三层，各管各的，混在一起做的话被攻击时会先误杀真实玩家。

### 第一层：Cloudflare

账号接口挂在 CF 后面，HTTP 层的洪水、TLS 握手、DDoS 清洗都归它。源站只听
`127.0.0.1:8080`，外面进不来。

**这一层我们不做任何事** —— 自己再写一套 HTTP 限流既挡不住洪水，又会在 CF
误判时给玩家添堵。

### 第二层：桥接门卫（应用层）

`11010` 是裸 TCP，CF 包不了，只能自己守。检查顺序就是成本顺序，贵的排在便宜后面：

```
黑名单 → IP 速率 → IP 并发 → 校验令牌（可能查库）→ 账号速率 → 账号并发 → 转发
```

原则只有一条：**不通过就一个字节都不回**。不是 403、不是 RST、不是错误页，
就是静默关闭。对方连「这个端口后面是什么」都问不出来。

- 坏令牌刷够 20 次/分钟 → 该 IP 拉黑 10 分钟（否则扫描器可以无限重试，零成本）
- 令牌校验结果缓存 60 秒 → 封号最多延迟一分钟生效。这是**刻意的取舍**：
  不缓存的话，一次洪水就能把 SQLite 打满，那时候不是「封得快」，是全都连不上
- 熔断只拒绝新连接，**已经在玩的人不会被踢**

### 第三层：熔断（保命）

| 档位 | 触发条件 | 动作 |
|---|---|---|
| 只停联机 | 每秒建连数持续 10 秒超过 `XCL_ACCEPT_RATE` | 关掉 11010 监听，HTTP 继续（玩家还能登录、看到维护提示）；流量下来后每 30 秒试探恢复 |
| 整个停服 | 内存超限 / 句柄用到 80% | 写 `cooling.json` 后 `exit 75` |

整个停服时端口无人监听，内核直接回 RST，**用户态零开销** —— 这是面对洪水唯一
有效的动作。systemd 因为 `RestartPreventExitStatus=75` 不会把它拉起来（被攻击时
立刻重启等于白给，CPU 全花在冷启动上），改由 `xcld-recover.timer` 每 2 分钟检查
冷却时间，到点再启动。

冷却时间按连续触发次数翻倍：5 → 10 → 20 → 40 分钟封顶。稳定跑满 10 分钟自动清零。

### 第四层（可选）：内核

`deploy/nftables-xcl.conf` 在 Go 之前就把超速的新连接丢掉。应用层再快也要先建
半连接、走一次 `accept`；只有内核能做到「包还没进进程就被丢」。

```bash
nft -f deploy/nftables-xcl.conf     # 应用
nft delete table inet xcl_guard     # 撤销
```

**不自动安装**，因为它可能和你已有的防火墙规则冲突，需要人工确认。

### 挂 Cloudflare（账号接口）

账号接口挂 CF 是合适的：HTTPS、防 DDoS、隐藏源站。但有两件事必须做对：

**① 桥接不能走 CF。** Cloudflare 免费版只代理 HTTP/HTTPS，任意 TCP 端口要 Spectrum（付费）。
所以 DNS 要拆成两条：

```
api.example.com     A    <源站 IP>   代理状态：橙云（Proxied）      → 反代到 :8080
bridge.example.com  A    <源站 IP>   代理状态：**灰云（DNS only）** → 直连 :11010
```

然后 `XCL_BRIDGE_ADVERTISE=bridge.example.com:11010`。
（也可以直接填源站 IP，但换了机器就得发版，域名 + 灰云更省事。）

**② 必须开 `--trust-forwarded`。** 走 CF 之后源站看到的来源 IP 全是 CF 的边缘节点，
不开这个参数，限流会把所有人当成同一个人 —— 一个人注册太快，全网都被拒。
开了之后从 `CF-Connecting-IP` 取真实 IP（只在前置确实是可信反代时才开，否则 IP 可伪造）。

### 运维

```bash
xcld user list                     # 账号列表（含封禁原因、最近登录）
xcld user ban <用户名> --reason 开挂
xcld user unban <用户名>
xcld user passwd <用户名>          # 不给 --password 就随机生成并打印
journalctl -u xcld -f
```

封号和改密码都会**同时作废该账号的全部登录态**，所以不存在「封了还能玩到令牌过期」。

## 安全边界（老实说）

挡得住：知道节点 IP 就起 easytier 上来扫网的人、爆破房间密码的脚本、批量注册的机器人。

挡不住：
- 有心人注册一个真账号然后乱来 —— 但那是可追溯、可封的，这正是要做账号系统而不是
  「设备令牌」的原因；
- 抓包拿到令牌的人：令牌是 bearer 凭据，TCP 那一层没有 TLS。要根治得给桥接加 TLS，
  代价是客户端要多一套证书逻辑，目前判断不值得；
- 密码忘了只能找管理员重置（刻意不做邮箱，见下）。

**为什么注册不用邮箱**：邮箱的全部价值是自助找回密码 + 防撞库，代价是要配 SMTP、
处理退信、防别人拿我们的接口发垃圾邮件。玩家里不少是学生，很多人没有
常用邮箱。表里留了 `email` 字段（可空），将来要加验证流程不用动已有数据。

**为什么注册要人机验证**：没有它，一个脚本可以在几分钟内把用户名占满。
Turnstile 优先，PoW 兜底 —— 因为客户端是 WebView2，Turnstile 有可能被判成异常环境，
国内网络访问 `challenges.cloudflare.com` 也不一定通。真过不去时改一行配置就能切到 PoW。

## 开发

```bash
go test ./...
go run ./cmd/xcld keygen                 # 生成密钥
XCL_SIGN_KEY=... XCL_POW_SEED=... \
  go run ./cmd/xcld serve --dir ./data --addr 127.0.0.1:8080
```

本地调试时 `--bridge` 给空字符串可以只起 HTTP，不占 11010。

### 压测门卫

限流阈值没法在线上试 —— 一试就是全站玩家陪你。所以有一个本机敲门工具：

```bash
go run ./cmd/xcl-loadtest -addr 127.0.0.1:11010 -n 300 -conc 20
```

它验证三件事：正常量级能过、超限被**静默丢弃**（零字节）、坏令牌刷够次数后
整个 IP 被拉黑。默认塞垃圾令牌，用来测拒绝路径；换 `-token` 成真令牌可以测放行。

`limit` 和 `guard` 两个包是纯函数（时间靠参数注入），不依赖网络，跑 `go test ./...`
就能覆盖全部分支 —— 这是「短期测不了」的主要解法。
