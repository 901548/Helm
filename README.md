# Helm

**带本地 AI Agent 的 SSH 终端工具 —— 你掌舵，AI 执行。**

Windows 桌面应用：多标签 SSH 终端、SFTP 文件管理、系统监控，以及一个能直接在远程服务器上逐步执行命令、完成自然语言任务的 AI Agent。基于 Tauri v2（WebView2），Win10/11 双击即用，无需安装浏览器或运行时。

## 功能特性

### 终端与连接
- **多标签 SSH 终端**：每会话一个 xterm.js 实例，流式直通 PTY，WebGL 渲染 + JetBrains Mono 等宽字体（CJK 回落系统雅黑）
- **多平台会话**：
  - Linux SSH（bash + POSIX）
  - Windows SSH（跳过 bash 专属注入，Agent 命令走 PowerShell EncodedCommand）
  - 远程桌面 RDP（生成临时 .rdp 拉起系统 mstsc 控屏，密码不落盘）
  - Docker 容器（kind=docker + 容器名，AI 命令经 `docker exec` 注入容器；P71 起也有容器内交互终端）
- **主机密钥 TOFU**：首次连接记录主机密钥（`known_hosts.json`，与 config.yaml 同目录），变更即拒绝并提示，防中间人；支持「忘记主机密钥」重置
- **连接超时 45s**：兼容慢认证（UseDNS/反向解析）的老服务器
- **断线重连横幅**：远端断开即时提示，一键重连

### 文件管理（SFTP 直传）
- 底部文件面板：浏览 / 新建目录 / 重命名 / 删除 / 上传 / 下载 / 文本预览编辑保存
- **面板 ⇄ 终端双向 cd 联动**：终端 `cd` 经服务器权威 PWD（OSC7 标记）同步面板；面板双击目录同步在终端执行 `cd`（仅 Linux 会话）
- **ZMODEM（rz/sz）文件传输**：终端内直接收发文件，进度浮层，落到下载目录
- 删除有根路径拒绝 + 危险命令判级双保险；下载有 64MB 上限、预览有截断标记

### 服务器快照（文件面板 🛡，让不可逆命令变可逆）
- **手动快照**：探测驱动的自适应打包范围；打包即排除 `shadow` / `ssh_host_*` / `id_*` / `*.pem` 等敏感文件（不加密反而更安全），完整性校验失败残包即删
- **命令前拦截**：后端行缓冲扣住回车，Critical 级命令停在 readline 缓冲里**未执行**就弹确认；拿不准的输入（↑历史 / Tab 补全）一律放行
- **滚动基线**：只读命令不触发，首次写操作立即拍，之后每 10 条写 / 15 分钟（仅 bash 会话）
- **还原**：预演 diff → 默认只解到暂存目录 → 就地还原需输入 `RESTORE` + 强制二次快照 + `sshd -t` 语法校验
- **变化包 + 快照链 + 单文件取回**：全量开链，`--newer-mtime` 增量随链叠加；包内清单浏览；单文件取回 / 就地还原自动 `.bak` 备份
- 快照落 `~/.helm-snapshots/`（避开 /tmp 定期清理），默认保留 10 份

### 系统监控
- 终端上方薄条：CPU / 内存 / 负载 / 网速，2 秒刷新，独立 exec 通道不污染终端
- Linux（/proc）与 Windows（PowerShell CIM）双模式采集

### AI 副驾驶（终端内嵌）
- **QA 模式**：流式问答，推理型模型（deepseek-r1 等）的思考过程单独实时显示
  - **只读命令协议**：模型可输出 `RUN: <命令>` 请求执行查看类命令（df / ss / journalctl 等 77 个白名单命令）辅助回答，自动执行且无确认；任何修改类命令被门禁拒绝并如实告知，**服务器零修改**（`RUN:` 行不进答案流，执行情况由步骤卡呈现）
- **Agent 模式**：自然语言任务 → AI 逐步生成命令 → 自动在活动会话执行 → 读输出与退出码回喂 → 直到输出 DONE
  - **长任务子目标化 + 失败恢复**：模型输出 `GOAL` 子目标与 `REFLEXION` 反思，按真实退出码判成败，失败自动重规划（不无意义重试）
  - **Function Calling 与文本协议双路径**：开 `agent_fc` 走结构化工具调用，模型不支持时自动粘性降级回文本协议
  - **内置技能短路**：磁盘/内存/日志/端口/sshd/用户/CPU/大文件 8 个常见运维意图命中即确定性执行（毫秒级，完全绕过 LLM 现场发挥）
  - **终端上下文感知**：提交时自动携带终端屏幕最近输出，"这是什么错误"类问题无需人工复述
  - cwd 随执行结果权威跟踪（`###HELM_PWD###` 标记回传）
  - Linux 走 bash，Windows 走 PowerShell EncodedCommand，Docker 会话走 `docker exec`
- **计划卡**：执行前展示整份命令清单 + 风险标记，可「查看命令 / 修改 / 执行 / 放弃」；纯安全计划自动执行免打扰
- **危险命令确认**：shell 分词 + 语义判级（Critical / Warning / Safe），覆盖 `rm -rf /`、`mkfs`、fork 炸弹、`$(...)` 命令替换、`xargs` 二次执行、执行器包装（`sh -c`/`python -c`）与绝对路径等绕过变体；Critical 强制确认，安全命令免打扰
- **多提供商**：任意 OpenAI 兼容服务（DeepSeek、Ollama、LM Studio 等），10 项预设 + 一等「自定义」快速填入 + 「测试连接」即时验证 + 「获取列表」拉取真实模型下拉选择；本地推理服务免 API Key
- **操作历史面板**：训练数据（用户输入 / AI 轨迹 / 技能金标）可视化审核，按类型/会话筛选、逐条展开、一键导出
- 支持自定义 `api_base_url` / `extra_headers` / `extra_body`，Agent 模式可用专属提示词

### 界面与安全
- **双主题**：白天 / 黑夜 / 跟随系统，xterm 终端同步换色
- **敏感信息 DPAPI 加密**：AI API Key 与会话密码以 Windows 用户级密文存储，明文永不下发前端
- **窗口尺寸 / 位置自动记忆**（含最小化离屏守卫）
- **终端设置即时生效**：字体大小、滚动缓冲行数
- **操作记录器**：终端输入与 AI 轨迹落 JSONL（按天滚动），用于训练数据采集，设置里可开关
- 设置持久化到 `config.yaml`（原子写入防损坏）

## 快捷键

| 快捷键 | 功能 |
|---|---|
| `Alt + I` | 聚焦终端内 AI dock 的输入框 |
| `Alt + L` | 开关 AI 全屏日志面板 |
| `Ctrl + F` | 终端内搜索 |
| `Ctrl + C` | 有选区时复制（无选区发送 SIGINT） |
| `Ctrl + V` | 粘贴 |
| `Ctrl + 滚轮` / `Ctrl + =` / `Ctrl + -` / `Ctrl + 0` | 终端字体缩放（临时，不持久化） |
| `Ctrl + Tab` / `Ctrl + Shift + Tab` | 标签页循环切换 |
| `Ctrl + 1..9` | 直达第 N 个标签 |
| `F11` | 全屏切换 |
| 鼠标中键 | 关闭标签页 |
| 会话行右键 | 连接 / 断开 / 编辑 / 删除 / 忘记主机密钥 |
| 状态栏「会话 N/M」 | 收起 / 展开会话栏（箭头指示方向） |

## 从源码构建

环境要求：Windows 10/11、Node ≥ 20、Rust（stable）、WebView2（系统自带）。

```bash
npm install          # 安装前端依赖
cargo tauri dev      # 开发模式(热重载)
cargo tauri build    # 打包 release:helm.exe + msi + nsis 安装包
```

产物位置：`src-tauri/target/release/helm.exe`、`src-tauri/target/release/bundle/`。

运行测试：

```bash
cd src-tauri && cargo test                          # 217 项(含 SSH/SFTP live 测试,无 src-tauri/config.yaml 时自动跳过)
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings   # 静态检查(零告警)
npm run build                                       # 前端构建
npm test                                            # 前端单元测试(Vitest 84 项)
```

> 项目使用 MinGW GNU 工具链开发（见 `.cargo/config.toml` 的 linker 配置），MSVC 工具链同样可行。

## 配置

配置文件查找顺序：命令行指定 > `./config.yaml` > `~/.config/helm/config.yaml`。
首次使用可将 `config.example.yaml` 复制为 `config.yaml` 按需修改（真实 `config.yaml` 不入库）。

```yaml
sessions:
  - name: my-server
    kind: linux            # linux(默认) | windows | rdp | docker
    host: 192.168.1.100
    port: 22
    user: root
    password: null         # 界面保存后为 DPAPI 密文(enc: 前缀),明文仅作首次输入
    key_file: null         # 私钥路径(~ 展开),密码优先
    container: null        # 仅 kind=docker 时填容器名/ID

ai:
  model: deepseek-chat
  api_key_env: API_KEY            # 环境变量回退名(界面固定写此值)
  api_key: null                   # 界面直填后存 DPAPI 密文,优先于环境变量
  api_base_url: null              # 自定义 OpenAI 兼容端点;本地 Ollama 等免 Key
  mode: qa                        # qa | agent
  temperature: 0.3
  max_tokens: null
  stream: true
  max_history: 30                 # 对话历史条数
  max_steps: 50                   # Agent 单任务最大步数
  max_output_chars: 6000          # 命令输出回喂模型上限
  timeout_secs: 60                # 单次 API 请求超时
  command_timeout_secs: null      # Agent 单命令超时(默认 60s)
  agent_confirm: false            # true = 所有命令都确认;false = 仅危险命令确认
  agent_fc: false                 # true = Agent 走 Function Calling(模型不支持时自动降级)
  ai_echo_terminal: false         # true = AI 命令/输出回显进终端
  system_prompt: |
    你是一个专业的运维AI助手……
  system_prompt_qa: null          # QA 模式专用提示词(空则用内置"直接回答"提示词,不沿用 system_prompt)
  system_prompt_agent: null       # Agent 模式专用提示词(不填则用内置默认)
  extra_headers: {}               # 附加请求头
  extra_body: null                # 附加请求体字段

ui:
  theme: dark            # light | dark | system
  window_width: 1200
  window_height: 800
  # window_x / window_y   # 窗口位置(物理像素,自动记忆,无需手填)
  sessions_panel_pct: 22 # 会话面板宽度%(遗留字段,仍生效)
  term_font_size: 14     # 终端字号(逻辑像素)
  term_scrollback: 5000  # 终端滚动缓冲行数
  recording_enabled: true  # 操作记录器开关
```

> AI 的 API Key 与会话密码建议经设置弹窗 / 会话表单填写，落盘自动加密，不要手写明文。

## 项目结构

```
frontend/src/
├── components/
│   ├── terminal/    # TerminalTabs / SysMonitor / AiCopilot
│   ├── sessions/    # SessionPanel / SessionForm
│   ├── files/       # FileBrowser / SnapshotPanel / SnapshotBrowser / RestoreModal
│   ├── chrome/      # StatusBar / SettingsModal / HistoryModal
│   └── ui/          # ModalShell(弹窗外壳:dialog 语义 + Esc + 遮罩关闭)
└── lib/
    ├── api.ts       # 桶导出
    ├── types.ts     # 全部 TS 类型
    ├── commands.ts  # invoke 封装
    ├── events.ts    # 事件订阅
    ├── constants.ts # 跨层契约常量(密文哨兵等)
    ├── osc.ts       # OSC7 PWD 提取(含单测)
    ├── paths.ts     # 路径规范化(含单测)
    ├── echo.ts      # AI 回显按会话重放(含单测)
    ├── conv.ts      # 对话记录→活动流卡片映射(含单测)
    ├── cdtrack.ts   # 非 bash 会话 cd 回退推断(含单测)
    ├── b64.ts       # base64 编解码(UTF-8 安全,含单测)
    └── aiSession.ts # AI 会话状态单桶(含单测)

src-tauri/src/
├── main.rs         # Tauri 入口 + 窗口尺寸/位置持久化
├── core.rs         # CoreState + commands + 事件负载 + 输出 poller
├── ai_job.rs       # AI 任务编排:run_ai_job 状态机 + 子目标/账本
├── task_exec.rs    # Agent 命令组装/解析/执行(Linux/Windows/Docker)
├── ssh.rs          # SshManager:连接/PTY/exec/SFTP 通道
├── agent.rs        # AI 客户端(引擎本体:QA 流式 + Agent 单步 + FC)
├── agent_prompts.rs # 提示词域(QA/Agent 内置提示词)
├── agent_stream.rs # 传输域(SSE 解析/重试退避/本地判定/models)
├── agent_parse.rs  # 解析域(回复清理/命令提取/子目标与 RUN 协议)
├── safety.rs       # 危险命令判级 + QA 只读命令门禁
├── skills.rs       # 内置技能(意图匹配 + 确定性命令序列)
├── fs.rs           # SFTP 文件命令
├── monitor.rs      # 系统监控采集(Linux/Windows)
├── recorder.rs     # 操作记录器(输入 + AI 轨迹落 JSONL)
├── snapshot.rs     # 服务器快照:探测/自适应/打包/校验/保留/链
├── intercept.rs    # 命令前拦截:行缓冲状态机(拿不准就放行)
├── baseline.rs     # 滚动基线:只读不拍,写操作才拍
├── restore.rs      # 快照还原:预演/暂存/强制二次快照
├── crypto.rs       # DPAPI 加解密
├── known_hosts.rs  # 主机密钥 TOFU
└── config.rs       # 配置加载/原子保存

scripts/            # 零依赖工具:bench/(评测) dataset/(SFT 导出) verify-ui.mjs(UI 冒烟) shot.mjs gen-icon.mjs
docs/               # 设计方案与审查记录 + screenshots/(UI 验证归档)
.github/workflows/  # CI:Vitest + build + cargo build + clippy(-D warnings) + cargo test
config.yaml          # 运行时配置(不提交)
```

## 技术栈

- **后端**：Rust + Tauri v2、russh / russh-keys / russh-sftp（SSH/SFTP 协议）、tokio、reqwest（AI HTTP 流式）、serde_yaml
- **前端**：Svelte 5（runes）+ Vite 6 + xterm.js 5（fit / search / webgl / web-links 插件）+ TypeScript + zmodem.js
- **安全**：Windows DPAPI（密文存储）、TOFU known_hosts、危险命令语义判级

## 训练数据与评测（可选）

操作记录器把终端输入与 AI 轨迹按天写入 `logs/terminal-YYYYMMDD.jsonl`（设置里可开关），配套三个零依赖脚本：

```bash
node scripts/dataset/export.mjs      # logs → SFT 数据集(dataset/sft-samples.jsonl + stats.json)
node scripts/bench/run.mjs           # Agent 评测:8 个真实运维任务,原生事件流判定 + 危险确认自动批准
node scripts/verify-ui.mjs           # UI 冒烟(需 CDP 调试端口 9229 + vite),退出码即结果
```

- **SFT 导出**：按 `ai_task` 分窗，只收 `exit_code == 0` 的成功步，剔除协议标记与重复；system 提示词运行时从 `agent_prompts.rs` 提取（改提示词无需同步脚本）。产物 `dataset/` 已 gitignore（含内网数据）
- **评测基准**：`scripts/bench/tasks.json` 定义 8 个任务（磁盘/内存/日志/服务/文件/用户/端口/清理），支持 `verify` 后置校验（file-exists / file-absent）与 `no-op` 判定
- 训练在训练机上进行（本机不训练）：`dataset/sft-samples.jsonl` → QLoRA 小模型 → GGUF → Ollama 导入 → 用同一套 bench 对比基线

## License

MIT
