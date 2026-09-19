# M3 · LLM 能力（碎碎念 / 对话）设计说明

> 上游 `whale-pet/dsh-pet` 的碎碎念与对话是**跑在 DSH 宿主里**的：provider/model 来自宿主的
> `agentDefaultModel`，API key 走宿主的 `credentials.resolve`，LLM 调用走 `ctx.llm.stream` 抽象。
> 本应用是**独立桌面应用**，没有宿主可依赖，所以这一层必须自己实现——
> 但**文案语义与交互照抄上游**，这样两只桌宠的性格与手感是一致的。

## 1. 出口标准与红线

- **默认完全离线**：新装的应用不会发起任何网络请求（`llm.enabled = false`）；
- 打开后可用：填 Key → 自检通过 → 碎碎念按周期出现、对话能一问一答；
- **密钥安全**：`config.jsonc` 里**永远不出现明文 Key**；日志里只允许出现打码形态；
- **失败不伪造**：出问题就说清楚原因（结构化 reason），绝不编一句假回复；
- 有**不依赖真实 Key** 的验证手段（本地 mock 服务端跑通端到端）。

## 2. 配置（`config.jsonc` 新增 `llm` 段）

```jsonc
"llm": {
  "enabled": false,                     // 总开关：false = 绝不联网
  "provider": "deepseek",               // deepseek | openai | ollama | custom
  "baseUrl": "https://api.deepseek.com",// provider=custom 时必填
  "model": "deepseek-chat",
  "temperature": 1.0,
  "timeoutSec": 60,
  "whisper": {
    "enabled": false,
    "intervalSec": 300,                 // 与上游 eventsRefreshSec.whisper 默认一致
    "persona": "…"                      // 人设；留空用内置默认（见下）
  },
  "chat": {
    "enabled": false,
    "memoryRounds": 5                   // 与上游 chatMemoryRounds 默认一致
  }
}
```

- **provider 预置**：`deepseek` → `https://api.deepseek.com`；`openai` → `https://api.openai.com/v1`；
  `ollama` → `http://127.0.0.1:11434/v1`（本地，不需要 Key）；`custom` 用 `baseUrl`。
- **Key 不放这里**：见第 3 节。`provider = ollama` 时允许没有 Key。
- 内置默认人设（按我们的角色改写了上游那句"Q版蓝发小女仆"）：
  「你是主人桌面上的 Q 版小鲸鱼桌宠，会时不时碎碎念一句。说话要自然随意、短短一句（20 字以内），
  温柔乖巧带点俏皮，说人话不啰嗦，不要解释你自己，不要提你是 AI。」

## 3. 密钥存储（`src-tauri/src/secret.rs`）

- 位置：`<应用数据目录>/llm-key.bin`，内容是 **DPAPI 加密后的密文**（`CryptProtectData`）；
- 绑定当前 Windows 用户：换账户/换机器解不开 → 返回可读错误，让用户重填，不崩溃；
- **不引入 `keyring` crate**：DPAPI 直接调 Win32 就够（见该文件头部注释里的取舍说明）；
- **日志纪律**：任何要打日志的地方用 `secret::mask()`（`sk-…abcd（共 N 字符）`）。

## 4. 请求形状（`src-tauri/src/llm.rs`）

- OpenAI 兼容：`POST {baseUrl}/chat/completions`，body `{ model, messages, temperature, stream:false }`；
- 客户端用 **`ureq` + `native-tls`**（只要"发一个 JSON、读一个 JSON"；`reqwest` 会带进 hyper/tower 一整套）。
  将来要做打字机流式再评估换实现 —— 届时只改 `llm.rs` 一处；
- **在 Rust 侧发请求**，所以页面 CSP 的 `connect-src` 不需要放行任何外部域名（少一个安全口子）；
- 超时：碎碎念 30s、对话 60s（与上游一致）；失败**不重试**（对话）以免重复计费；
  网络类错误给结构化 reason：`offline / timeout / unauthorized / rate-limited / bad-response / no-key / disabled`；
- 消息构造：`[{role:"system", content: persona + "\n你的名字是\"X\"。"}, …最近 N 轮…, {role:"user", content}]`。

## 5. 记忆（`memory.json`）

- 结构（比上游少一层 `assetRoot`，因为本应用只有一个素材根）：
  `{ "<宠物 id>": { "messages": [ { "role": "user|assistant", "content": "…", "ts": 1700000000000 } ] } }`
- **全量保存不删**；送进上下文的是最近 `memoryRounds * 2` 条（1 轮 = 1 问 1 答）；
- 损坏处理照抄上游：解析失败 → 备份成 `memory.json.bak-<时间戳>`（备份失败只告警）→ 重建空记忆；
- **写盘串行化**：一把锁 + 队列，避免"碎碎念与对话同时写"把文件写坏。

## 6. 交互（照抄上游的极简形态）

- **碎碎念**：到点生成一句 → 走**已有的气泡窗**显示（10 秒后自动消失，与上游 `BUBBLE_DURATION_MS` 一致），
  同时可以随机播一条 `animations.events.whisper` 动画（有该事件槽时）；
- **对话**：不做常驻聊天面板。在宠物右上角弹一个**单行自适应输入框**（回车发送、Esc 关闭、失败红字留在框内），
  回复同样走气泡显示 —— 与上游一致，也符合"桌宠"的形态（不占屏幕、不抢注意力）；
- 输入长度上限 2000 字（上游同值），空消息直接拒绝。

## 7. 验证策略（不依赖你的 Key）

1. **本地 mock 服务端**：`tools/mock-llm`（一个几十行的 Rust bin 或 PowerShell `HttpListener`）监听
   `127.0.0.1:端口`，实现 `POST /chat/completions`，返回固定的 OpenAI 兼容响应；
   把 `provider=custom` + `baseUrl=http://127.0.0.1:端口` 指过去，就能端到端验证：
   请求形状（system/user 组装、温度、model）、响应解析、记忆写入、气泡显示、错误分支
   （401 / 超时 / 空文本 / 非法 JSON）；
2. **默认离线自检**：不配置任何 provider 时，应用启动到退出**零网络请求**
   （用一个"只监听、不应答"的探针端口验证不会有连接尝试，或直接检查代码路径 + 日志）；
3. **密钥安全自检**：保存后读 `config.jsonc` 与日志，确认没有明文；`llm-key.bin` 用文本方式打开是乱码；
4. **真机联调**（可选，需要你提供 Key）：填 DeepSeek 或本地 Ollama，跑一次碎碎念 + 一次对话。

## 8. 分步落地顺序

1. 配置段 + 密钥存储 + 自检命令（`llm_selftest`）；
2. `llm.rs`：请求构造 / 响应解析 / 错误映射（先用 mock 服务端打通）；
3. 碎碎念：定时器 + 气泡；
4. 对话：输入框弹窗 + 记忆读写；
5. 设置窗口新增「AI」页（开关 / provider / model / Key / 自检 / 隐私说明）；
6. 文档与验证记录（VERIFICATION 第 13 节），release 二进制回归一次。
