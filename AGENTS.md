# Helm — 项目档案 (AGENTS.md)

> 本文件是项目的**唯一可信记忆源**。每次会话开始时必须先读取本文件恢复上下文。
> 每完成一个阶段后更新"阶段清单"与"坑位记录"。不要依赖对话历史。

## 1. 项目全貌

**定位**:带本地 AI Agent 的 SSH 终端工具——"你掌舵,AI 执行"。
**目标平台**:Windows(10/11 自带 WebView2,双击 exe 即用,无需装浏览器)。

### 技术栈演进史(重要)
- **旧架构(winit + wgpu + ratatui)**:窗口用 winit,画布用 ratatui 字符画经 wgpu 渲染。CPU 高(主循环 100ms 无条件全量重绘),UI 上限是 ANSI 字符画。
- **新架构(Tauri v2 + Svelte + xterm.js)**:窗口换 Tauri(WebView2),界面用 Svelte + HTML/CSS 重建,终端用 xterm.js 流式直通。**旧 `src/`(ratatui 版)已随 Phase 2 删除。**
- **当前状态**:处于 **Phase 5 完成**(Phase 3 前端 MVP、Phase 4 验证打包、Phase 5 全量对齐均已落地,`cargo build`/`npm run build`/`cargo tauri build` 通过)。P11-P34 均为开发迭代(未打包)。**当前功能全集**:多标签 SSH 终端(xterm.js 直通)、**多平台会话(P32/P33:SessionInfo 增 `kind` = linux(默认)/windows/rdp,表单平台下拉,L/SessionPanel 类型徽标;Windows SSH 会话跳过 OSC7 注入与前端 PWD/cd 联动;RDP 会话经 `rdp_connect` 生成临时 .rdp 拉起系统 mstsc 控屏,密码不落盘由系统提示输入)**、文件浏览器(SFTP 直传,**跨平台(经 SFTP canonicalize 天然风格化),rdp 会话显示占位,面板→终端 cd 同步仅 Linux**)、系统监控薄条(2s 轮询,**P34 起 linux/windows 双模式采集**)、**终端内嵌 AI 副驾驶**(P25:Alt+I 终端任务输入 + AI 命令/输出回显 + AI 状态条 + 危险命令确认 + Alt+L 日志,取代 P17 悬浮窗;**P31 起空闲隐藏、状态栏点文字切模式**)、Agent 自动执行(**run_task_exec 锁会话 + cwd 跟踪 + 退出码回喂,仅危险命令确认**,P25,**P34 起 Linux bash / Windows PowerShell(EncodedCommand)双分支执行**)、双主题(白天/黑夜/跟随系统)、设置持久化(P13 起 API Key 可直填,DPAPI 密文存储;**P26-6 起窗口尺寸/位置自动记忆**;**P48 起任意 OpenAI 兼容提供商 10 项预设快速填入 + 测试连接按钮即时验证,本地推理服务(Ollama 等)免 Key**;**P49 起模型名不必手猜:「获取列表」按钮拉取 GET /models 真实模型下拉选择**)、会话密码 DPAPI 加密存储 + 主机密钥 TOFU(P26)。最新权威实现以 §2/§4 为准,演进史见 §5。

### 目录结构(当前实际,P42 按域模块化)
```
F:\Helm\
├── AGENTS.md             ← 本档案
├── README.md / .gitignore
├── package.json          ← 前端 (Svelte + Vite + xterm.js)
├── vite.config.ts
├── frontend/             ← 前端源码 (vite root)
│   ├── index.html
│   └── src/              ← App.svelte / main.ts
│       ├── components/   ← P42 按域分组
│       │   ├── terminal/   ← TerminalTabs / SysMonitor / AiCopilot
│       │   ├── sessions/   ← SessionPanel / SessionForm
│       │   ├── files/      ← FileBrowser
│       │   └── chrome/     ← StatusBar / SettingsModal
│       └── lib/
│           ├── types.ts     ← 全部 TS 类型(P42 自 api.ts 拆出)
│           ├── commands.ts  ← invoke 封装
│           ├── events.ts    ← 事件订阅
│           └── api.ts       ← 桶导出(现有 "../lib/api" 导入零改动)
├── src-tauri/
│   ├── Cargo.toml        ← Rust 核心 + tauri
│   ├── tauri.conf.json
│   ├── build.rs
│   ├── icons/
│   └── src/
│       ├── main.rs       ← Tauri 入口 (纯 bin,无 lib.rs)
│       ├── core.rs       ← CoreState + 薄命令层 + 事件负载 + poller(P42 起 AI 编排/任务执行已拆出)
│       ├── ai_job.rs     ← AI 任务编排:run_ai_job 状态机 + task_exec + TaskCtx(P42 自 core.rs 拆出)
│       ├── task_exec.rs  ← Agent 命令执行域:双平台组装/编码/解析/run_task_exec + 测试(P42 自 ssh.rs 拆出)
│       ├── ssh.rs        ← SshManager:连接/PTY/exec/SFTP 通道管理
│       ├── crypto.rs     ← DPAPI 加解密(P13 新增)
│       ├── monitor.rs    ← 系统监控采集(P16 新增)
│       ├── fs.rs         ← 文件浏览/管理命令(P17 新增)
│       ├── config.rs / agent.rs / safety.rs / known_hosts.rs
├── scripts/              ← gen-icon / shot / reload-shot / eval(CDP 验证工具)
├── docs/screenshots/     ← UI 验证截图归档(P41)
├── config.yaml           ← 运行时配置(known_hosts.json 同目录,P41 勘误)
├── assets/fonts/CascadiaMono-Regular.ttf  ← 终端字体(Phase 3 前端引用)
└── .cargo/config.toml    ← linker 指向 MinGW gcc(源码根,cargo 从 src-tauri 上溯找到)
```
> 说明:前端源码固定在 `frontend/`(vite root 已在 vite.config.ts 配置)。

## 2. Rust 核心(保留模块,不要改动语义)

### `config.rs`
- `SessionInfo { name, kind(P32/P33:SessionKind 枚举 lower 值 `linux`(默认)/windows/rdp,serde default,旧配置无 kind 自动 linux), host, port(default 22), user(default root), key_file: Option, password: Option }`
- `AiConfig { model, api_key_env, api_key: Option<String>(DPAPI 密文, P13 新增), api_base_url, system_prompt(自定义提示词), system_prompt_agent: Option<String>(Agent 模式专用提示词, P25 新增), temperature(0.3), max_tokens, stream(true), max_history(30), max_steps(50), max_output_chars(6000), timeout_secs(60), command_timeout_secs: Option<u64>(Agent 单命令超时, 0/None → 60, P25 新增), agent_confirm(false, P25 起默认 false,危险命令始终确认), mode("qa"), extra_headers, extra_body }`
- `UiConfig { window_width(1200), window_height(800), dock_sessions(true), dock_chat(true), sessions_panel_pct(22), chat_panel_pct(28), theme(默认 Dark) }`;`Theme` 枚举(serde lowercase `light|dark|system`,默认 `Dark`)
- `HelmConfig { sessions: Vec<SessionInfo>, ai: Option<AiConfig>, ui: Option<UiConfig> }`
- `load_config_with_path(cli) -> (PathBuf, HelmConfig)`:查找顺序 命令行 > `./config.yaml` > `~/.config/helm/config.yaml`(dev 模式 CWD 是 src-tauri,加载失败回退 `../config.yaml`)
- `save_config(path, config)`;`expand_tilde(path)`

### `ssh.rs` SshManager(关键公共 API)
> **P26-4c 起全部方法改 `&self` + async**,内部 `sessions: Arc<Mutex<HashMap<String, Arc<Mutex<SshSession>>>>>` + `active: Arc<Mutex<Option<String>>>` + `notify: Arc<Notify>`;map 锁瞬时持有,网络操作拿到 Arc handle 后锁外执行。
- `SessionStatus { Connected, Disconnected, Connecting }`
- `mark_connecting(name)` / `connect(info) -> Result<bool>`(45s 超时(原 15s,P28:OpenSSH 7.4 服务器认证需 ~20s,15s 必误报失败),密码优先(P26-2 DPAPI 解密),否则私钥)/ `open_shell(name, cols, rows, kind)`(10s 超时,PTY + shell;**P24 注入不可见 OSC7 标记**:shell 建好后经 cmd_tx 发 `stty -echo; export PROMPT_COMMAND='printf "\033]7;helm:%s\a" "$PWD"'; stty echo; clear`,bash 每次重画提示符都发 `ESC]7;helm:$PWD BEL`,终端不渲染,为前端文件面板 cd 跟随提供权威 PWD;**P32 起仅 `kind==Linux` 注入**,windows 会话跳过否则 cmd/PowerShell 报错污染终端;**P26-3 TOFU**:SshHandler 实现 ServerCheck,host key 与 known_hosts 不符则连接失败提示)
- `send_input(name, bytes)` / `send_active_input(bytes)` / `send_resize_all(cols, rows)`
- `drain_output(name) -> Option<Vec<u8>>`(try_recv 累积;现由 `spawn_output_poller` 事件驱动 → emit `terminal-output`)/ `wait_output()`(P26-4c:等待下一次 Notify,供 poller 休眠)
- `exec_handle(name) -> Option<Arc<client::Handle<SshHandler>>>`(独立 exec 通道,不与交互 shell 共享,供 sysmon 用)/ `sftp_handle(name) -> Option<Arc<SftpSession>>`(P26-5:懒建缓存 SFTP 会话)
- `execute(name, cmd) -> Result<String>`(30s 超时,收集 stdout,空则 stderr;P26-4a 输出超 8MiB 停累积)
- `disconnect(name)` / `remove(name)` / `disconnect_all()` / `get_status(name)` / `active_name()` / `session_names()` / `set_active(name)` / `clear_active()`
- **Agent 任务执行(P25,P34 起双分支)**:`task_exec_cmd(cwd, cmd)` 组装 `cd '<cwd>'; { <cmd>; } 2>&1; rc=$?; printf '\n###HELM_END###\n###HELM_EXIT###%s\n###HELM_PWD###%s\n' "$rc" "$PWD"`(cwd 单引号转义 `'\''`);`task_exec_cmd_windows(cwd, cmd)` **经 `powershell -NoProfile -NonInteractive -EncodedCommand` 调用**(UTF-16LE base64,`encode_powershell`),脚本 `Set-Location -LiteralPath '<cwd>'; { <cmd>; } 2>&1; $rc=$LASTEXITCODE; if($null -eq $rc){$rc=0}` + `###HELM_*###` 标记(格式与 Linux 一致,cwd 单引号转义 `''`);`parse_task_output(raw) -> (output, exit_code, pwd)`(无标记 → 原样,-1/空);`TaskExecResult{ output, exit_code, pwd }`;`run_task_exec(handle: Arc<client::Handle<SshHandler>>, cmd, cwd, timeout_secs, kind: SessionKind) -> Result<TaskExecResult>`(最小超时 1s,读 Data/ExtendedData 累积,超时返回错误;**P26-4a 输出 64KiB trim_to_tail**);`kind_of(name)` 返回会话 kind(未连接默认 Linux)。含 5 个纯函数单测 + P26-4a trim_to_tail 单测 + P34 Windows 2 个单测)

### `agent.rs` Agent
- `Agent::new(&AiConfig) -> Result`(校验 model;API Key 优先 api_key DPAPI 解密,否则 api_key_env 环境变量,失败即报错;P13 新增;**P25 按模式选提示词** `system_prompt_for(mode, system_prompt, system_prompt_agent)`)
- `chat(user_input, sink: Option<&mut dyn FnMut(&str)+Send>) -> Result<String>`(QA 模式,流式增量回调)
- `agent_step(task, prev_output, ctx, sink) -> Result<String>`(Agent 模式单步;`ctx` 为环境描述如 `会话 1 (root@192.168.79.150)，当前目录 /opt`,首轮 prompt = "当前环境：{ctx}\n新任务：…",后续 = "当前目录：{ctx}\n这是上一步命令的执行输出…";返回命令或 "DONE",首调记录 current_task;P25 新增 ctx)
- `max_steps()` / `max_output_chars()` / `agent_confirm()` / `command_timeout_secs()`(0/None → 60)/ `clear_history()` / `reset_task()` / `set_mode(mode)` / `mode()` / `current_task()`
- 模块级:`parse_commands(text) -> Vec<String>`、`truncate_text(text, max)`

### `safety.rs`
- `DangerLevel { Safe, Warning, Critical }`;`check_danger(cmd) -> (DangerLevel, String 原因)`
- **P29 起为 shell 分词 + 语义判级**(旧版子串正则已重写):新增 `split_commands`(按 `;`/换行/`&&`/`||`/`|` 引号感知切段)+ `tokenize_segment`(引号感知分词)+ `first_command`(跳过 sudo/env/time/nohup/nice/setsid/stdbuf 包装器与前导 `-` 选项);Critical 段级扫描(rm 递归+强制+`/` 前缀目标 / shutdown / reboot / mkfs / dd of=/dev/* / fork炸弹 / chmod 777+`/` 前缀),任一段 Critical 即整体 Critical,否则任一 Warning
- Critical: rm -rf 根目标、shutdown、reboot、mkfs、dd到/dev、fork炸弹、chmod 777 /
- Warning: rm -rf(非根)、chmod 777、pkill、killall、fdisk、parted;**关停/进程类用 `first_command`(命令名判级,`echo shutdown` 不误报);`echo 'rm -rf /'`(引号整体参数)Safe**

### 旧 app.rs 已迁移语义(现位于 core.rs,勿改动语义)
- **连接流程**:后台 spawn `connect → open_shell(cols,rows) → set_active`,完成发 `connection` 事件(`Connected{name}` / `Failed{name,error}`)
- **AI 流程**:后台任务按 mode 分发;QA 直接 `chat` 流式;Agent(P25 起)逐步 `agent_step → parse_commands → check_danger → task_exec(锁会话 run_task_exec)`,`agent_confirm || level!=Safe` 才发 `PendingCommand` 等 `AiControl::{Approve,Reject,Cancel}`;cwd 随 `###HELM_PWD###` 回传跟踪、退出码判成功/失败回喂 next agent_step
- **事件枚举**:`AiPayload::{StepBegin, Streaming{text}, StepOutputEnd, CommandStep{command,success,message,output}(output 为截断 2000 的模板外输出, P25), Done{message}, Error{message}, PendingCommand{command,level,reason}, Busy{busy}}`;`AiControl::{Approve,Reject,Cancel}`
- **P25 TaskCtx**:`core.rs` `TaskCtx{ name, host, user, pwd }`(Clone);`ai_submit(app,state,input,pwd: Option<String>)` 在 Agent 模式锁定活动会话(无则报错"未连接会话…"),`pwd` 来自前端 OSC7 权威 PWD 作初始目录,经 `run_ai_job` 的 `ctx` 注入 agent_step

## 3. 旧 UI(已删除,仅供理解)
- `ui/theme.rs`:旧暗色板(BG #1A1A2E 等)。**新前端用明亮主题**,变量见 §4
- `ui/terminal.rs` TerminalPanel + `ui/term/*` 自研 ANSI 模拟器(44 项测试):已由 **xterm.js** 替代,`ui/term` 随旧 src/ 删除
- `ui/sessions.rs` / `ui/chat.rs` / `ui/settings.rs`:Phase 3 用 Svelte 组件重建

## 4. 前端(Svelte + xterm.js,已建成)
- 组件树:`App` → `SessionPanel`(会话栏:完整列表/状态圆点/折叠) · `SessionForm`(新建/编辑弹窗) · `TerminalTabs`(多标签,每会话一个 xterm 实例) · `SysMonitor`(终端上方薄条) · `FileBrowser`(底部文件面板) · `AiCopilot`(终端内嵌 AI 状态条,P25) · `SettingsModal` · `StatusBar`
- **两区布局(P17+P18, P31 结构清理)**:`.app`(flex column)→ `.main-row`(flex row,`.body` 冗余层已删,直接为 app 子级 = 会话栏 `width: var(--sp,22%)` + 右 `.main-col` flex column 主列)→ `StatusBar`。主列 = `.terminal-pane`(`flex:1`,内含 tabbar+**SysMonitor 薄条(P18 回归,~23px)**+**AiCopilot AI 状态条(P25,~28px,P31 起条件渲染)**+term-area)+ `.fs-panel`(底部文件面板,`flex-shrink:0`,默认 **折叠 32px(P31)**,`.fs-resize` 顶部 4px 拖拽调高 80-600px)。会话栏折叠 44px(P18 折叠态 header 竖向 + 大 `▶` 展开按钮);`--sp` 由 App `style:` 绑定 `sessionsPanelPct`。
- **AI 内嵌副驾驶(P25,取代 AiFloat 悬浮窗;P31 起条件渲染)**:`AiCopilot.svelte` 渲染于 TerminalTabs 的 SysMonitor 与 term-area 之间的 AI 状态条(QA/Agent 模式切换 + 状态文案 + 停止/日志/清空/⌥I 任务按钮),**P31 起仅 `aiBusy || aiPending || aiTaskOpen` 时渲染,空闲隐藏以让终端空间最大化**;模式切换入口由 `StatusBar` 承担(点击 AI 状态文字循环切换,P31);`Alt+I` 在活动终端底部弹 `.ai-task-wrap` 任务输入行(Enter 提交、Esc 取消,`bind:focus` 自动聚焦;busy 禁用);提交走 `onTaskSubmit(name,text)` → App `submitTask` 带 `pwds[name]`(OSC7 权威 PWD 经 TerminalTabs `onPwd → App pwds` 记录)调 `api.aiSubmit`;AI 事件回显进活动终端(`pushEcho → aiEcho{seq,name,text}` → TerminalTabs `$effect` `term.write`,QA streaming 首 chunk 前缀 `[AI]`,commandStep 前缀 `[AI 执行/失败]` ANSI 色,done 绿/error 红);`Alt+L` 开关 `.ai-log-wrap` 全屏日志面板(新条目自动滚底);危险命令 PendingCommand 在状态条内插 `⚠ badge + 命令 + 执行/跳过/取消` 按钮 → `api.aiControl`。
- **明亮主题**:CSS 变量统一管理(`--bg --panel --border --text --accent` 等),替代旧 theme.rs 暗色板
- **双主题(P12 主题系统)**:默认**黑夜**(`--bg #0f1116` 等,`data-theme` 无值即暗色),`[data-theme="light"]` 覆盖层为白天。App 里 `theme` 状态(`light|dark|system`)→ `resolvedTheme` derived → `$effect` 写 `document.documentElement.dataset.theme` + `colorScheme`,并监听 `prefers-color-scheme` 动态跟随系统。共享变量 `--hover --active-bg --input-bg --modal-bg --track-bg --bubble-bg --warn-bg --term-bg --term-fg --tabbar-* --statusbar-*`,所有组件硬编码色已抽成 var。设置弹窗"界面"tab 有"主题"下拉(白天/黑夜/跟随系统),随 `ui.theme` 持久化。TerminalTabs 收 `theme` prop,light/dark 两套 xterm theme(含浅底 ANSI 调色板),`term.options.theme` 即时重绘。
- 终端:每个会话一个 `new Terminal()`,`fit` addon 处理 resize → `invoke('resize_sessions')`,`onData` → `invoke('send_active_input')`,`terminal-output` 事件 → `term.write()`;终端字体 `"Cascadia Mono"`(assets/fonts 复制到 `frontend/src/fonts/`)
- 增强(CSS 层,无需 Rust):Ctrl+F 搜索(@xterm/addon-search)、Ctrl+C 复制选区、Ctrl+V 粘贴、选区自动复制、中键关闭标签、会话行右键菜单(连接/断开/编辑/删除);快捷键 **⌥I = AI 任务输入、⌥L = AI 日志**(P25);危险命令确认在 AI 状态条(执行/跳过/取消,P25)
- **面板⇄终端 cd 联动(P24 权威 PWD = OSC7)**:**终端 → 面板**:`open_shell` 注入的 PROMPT_COMMAND 使 bash 每次重画提示符发 `ESC]7;helm:$PWD BEL`;TerminalTabs `feedEcho` 在 `stripAnsi` **前** `extractOscPwd` 提取标记(跨 data 块保留未终止 `ESC]` 片段)→ `syncOscPwd`(剥尾斜杠 + `lastPwd` 去重,普通命令 PWD 不变不触发)→ `fireCd` → 回调 `onCd(name,pwd)` → App `handleCd`(cdSeq 自增 → `cdReq` 状态)→ FileBrowser `$effect` 命中 activeTab 才 `refresh(target)`(内部 `refreshSeq` last-write-wins 防竞态:两次 invoke 后过期结果直接丢弃)。**面板 → 终端**:双击文件夹/`↑` 上一级用 `api.sendInput(name, \`cd ${shq(path)}\r\`)` 直通后端写 PTY(不经 xterm onData、不走 cdReq,避免二次 refresh),远端执行并回声,OSC 再读回真实 PWD 双向同步。`resolvePath` 剥尾斜杠(除根)拼绝对路径;fs.rs SFTP `read_dir` 天然不含 `.`/`..`(导航交给 goUp);非 bash 会话(无 OSC 标记)回退 pendingCd 回显确认(见 §5 P23/P24)。**P32:此联动仅 Linux 会话生效**——TerminalTabs 收 `kinds` prop,`feedEcho` 对 windows 会话开头即 return(跳过 cd 跟踪);SysMonitor 对 linux/windows 均渲染(P34 起采集按 kind 分支);FileBrowser 收 `kinds`,**P34 起 windows 会话解锁 SFTP 面板**(路径分隔符/驱动盘导航已 kind 化),仅 rdp 会话显示占位。
- `lib/api.ts` 集中封装:TS 类型(SessionInfo/SessionStatus/AiConfig/UiConfig/DangerLevel/AiPayload/ConnectionPayload/TerminalOutputPayload/SysMonPayload/FileEntry/FsResult/AiLogEntry)+ 全部 invoke + 事件订阅(onConnection/onAi/onTerminalOutput/onSysMon);`aiSubmit(input, pwd?)`(P25 加 pwd 参数传 Agent 初始目录)

## 5. 阶段清单与进度
- [x] **Phase 0** 新建 AGENTS.md 完整档案(当前阶段已完成)
- [x] **Phase 1** Tauri 脚手架:Cargo.toml 加 tauri、build.rs、tauri.conf.json、icons、vite+Svelte 前端骨架(**已完成**,`cargo tauri dev` 空窗口验证通过)
- [x] **Phase 2** Rust 核心落地:CoreState + commands(会话CRUD/连接/输入/调整/输出推送)(**已完成**,config/ssh/agent/safety 迁移至 src-tauri + core.rs 命令层;`cargo build` 与 `cargo test` 13 项全过,含真实 SSH 集成测试;旧 `src/` ratatui 版已删除,前端目录固定 `frontend/`)
- [x] **Phase 3** 前端 MVP:会话列表 + 多标签 xterm + 状态栏,明亮主题(**已完成**,全部组件落地,npm run build 通过)
- [x] **Phase 4** 验证:冒烟(cargo tauri dev 窗口存活/UI 渲染/vite 服务)+ `cargo tauri build` 出 release exe 与 msi/nsis 安装包(**已完成**)
- [x] **Phase 5** 全量对齐:AI 流式聊天 / 设置弹窗 / 右键菜单 / 搜索 / 选区复制 / 危险命令确认(**已完成**)
- [x] **Phase 6** 修复收尾(开发阶段,未打包):P1 capabilities 权限修复 / P2 字体加载 / P3 QA 停止 / P4 设置保留 extra 字段 / P5 dock 开关生效 / P6 改名先断开 / P7 远端断开同步 / P8 终端 Ctrl+C 选区保护 / P10 搜索框 a11y / dead-code 清理(**已完成**,`cargo build`+`cargo test` 13 项+`npm run build` 通过)
- [x] **P11 白屏修复**:App.svelte `style:--sp {sessionsPanelPct}` 简写语法缺 `=` 导致编译产物引用不存在的 `sp`/`cp` 变量,App 挂载即抛 `ReferenceError` 白屏。改为显式绑定并补 `%`:`style:--sp={\`${sessionsPanelPct}%\`}`(**已完成**,`npm run build` 通过 + 冒烟截图 194 色桶确认渲染,非白屏)
- [x] **P12 主题系统**:双主题(白天/黑夜/跟随系统),config.rs `UiConfig.theme` + 前端变量化 + xterm 双主题(**已完成**,`cargo build`+`cargo test` 13 项+`npm run build` 通过,CDP 截图验证两主题各区域配色正确)
- [x] **P13 API Key 直填 + DPAPI 存储**:设置弹窗 AI tab 加 API Key 密码框(`type="password"`),明文经 `crypto.rs` DPAPI 加密存 `config.yaml` 的 `ai.api_key`(base64 密文),优先于 api_key_env;`get_ai_config` 回传哨兵 `"·"` 脱敏(密文永不下发),`update_ai_config` 空串/哨兵保留原密文;`Agent::new` 先解密 api_key 否则回退 env(**已完成**,`cargo build`+`cargo test` 18 项(原 13+5 新增)+`npm run build` 通过;端到端验证:填 `sk-test-direct-12345` → config.yaml 落 base64 DPAPI 密文(非明文),重开哨兵 placeholder"已保存(留空则不修改)",空保存保留密文,重启 Agent::new 解密无报错)
- [x] **P14 布局重构(左 rail + 底部聊天)**:`.main-row` grid `var(--rail,52px) 1fr`,ChatPanel 移到底部(`flex: 0 0 var(--cp,28%)` + 折叠按钮 ▾/▴),SessionPanel 重写为 52px 窄 rail(＋ 按钮 + 竖排状态圆点)+ hover flyout(230px,modal-bg,内含会话列表/右键菜单);`--sp`/`sessionsPanelPct` 移除(`sessions_panel_pct` 字段保留透传),`:root` 加 `--rail: 52px`(**已完成**,CDP DOM 验证:rail 52x551 / terminal-pane 52,0 1148x551 / chat 0,551 1200x214 / statusbar 0,766 1200x34;flyout hover 开合 + 折叠按钮经 CDP 事件模拟验证)
- [x] **P15 经典三栏布局 + 视觉打磨**:用户反馈底栏聊天"很怪"+ 三明治感 + 配色/间距/圆角细节别扭,取消 P14 底部聊天,回归经典三栏(VS Code 风):`.body` 改 `flex-direction: row`;`.main-row` 改 flex 三栏 = 会话栏(`width: var(--sp, 22%)`,默认完整列表:状态圆点+名称+user@host,头部折叠按钮,折叠后 44px)+ 终端(`flex:1`)+ 聊天栏(`width: var(--cp, 28%)`,border-left,折叠后 36px 竖条仅留 QA/Agent/折叠键);恢复 `sessionsPanelPct` 状态;`--rail` 变量移除;`--radius/--radius-sm/--radius-lg` 圆角变量收敛,SessionPanel/ChatPanel/SettingsModal/SessionForm/TerminalTabs 硬编码圆角改 var;设置弹窗界面 tab 恢复"会话面板宽度%"、"AI 面板宽度%"两个宽度输入(**已完成**,`npm run build`+`cargo build` 通过;CDP 验证:三栏 rect 264/600/336 + statusbar 34,会话栏折叠 44px 隐藏列表、聊天栏折叠 36px 隐藏输入,右键菜单、设置弹窗宽度字段均正常;截图像素扫描确认三栏配色 #171B22/#0A0D11/#171B22)
- [x] **P16 系统监控条(终端上方)**:连接后显示 Linux CPU/内存/负载/网速,每 2s 刷新。**采集走独立 exec 通道**(`SshManager::exec_handle(name)` 返回 `Arc<client::Handle<SshHandler>>`,与交互 shell 并存,不经 PTY,不污染终端输出),仿 core::exec_command 的 `run_exec(handle, cmd)`。`monitor.rs`(新):`collect_command()` 用分隔符 SEP1-4 一次取回 `/proc/stat`+`/proc/meminfo`+`/proc/loadavg`+`/proc/net/dev` 四段;纯函数 `parse_stat/parse_meminfo(优先 MemAvailable 回退 MemFree+Buffers+Cached)/parse_loadavg/parse_netdev(跳过 lo)/parse_sample`;`SysPrev{ cpu_total,cpu_idle,rx,tx,last_time }` 相邻采样差值算 CPU%/网速(Instant 无 Default,手动 impl Default);`spawn_sysmon_poller`(setup 里 `tauri::async_runtime::spawn`,2s tick,锁外 run_exec + 5s timeout,`app.emit("sysmon", payload)`);payload camelCase `{ name,cpu,memTotalMb,memUsedMb,load1,load5,load15,rxBps,txBps,ok,error }`,采集失败 ok=false。前端:`api.ts` 加 `SysMonPayload` + `onSysMon`;新 `SysMonitor.svelte`(收 `activeTab` prop,onSysMon 仅 `p.name===activeTab` 才更新;CPU/MEM 迷你进度条+数字、负载三值、网速 `↑tx/↓rx` 格式化;ok=false 显示 `—`+错误;高约 26px,`--term-bg` 底 + `border-bottom`);TerminalTabs 在 `.tabbar` 与 term-area 之间插 `<SysMonitor {activeTab}/>`(**已完成**,`cargo build`+`cargo test` 23 项(原 18+5 新增)+`npm run build` 通过;CDP 冒烟:192.168.79.150 离线不可达,改经 `window.__TAURI_INTERNALS__.invoke('plugin:event|emit',{event:'sysmon',payload})` 注入假数据,验证 ok 路径渲染 CPU 43%/MEM 3.0G/7.8G/负载/网速、ok=false 路径显示 `—`+错误文案;后续被 P17 调整,见下)
- [x] **P17 布局重构(会话栏+主列+底部文件)+ AI 悬浮窗 + 文件浏览器**:用户反馈"三块并排"不舒服、底部想做文件展示、AI 对话没合适位置、P16 监控条位置都不满意。最终方案:**两区布局**(`.main-row` flex row = 左会话栏 `var(--sp)` + 右 `.main-col` flex column 主列),主列 = 终端 `flex:1` + 底部文件面板(`flex-shrink:0`,默认 180px,拖拽调高 80-600px,折叠为 32px);**ChatPanel 删除**,AI 对话重构为 `AiFloat.svelte` 悬浮窗(右下角气泡按钮,点开变 340x460 聊天窗,可拖拽/缩放,未读/忙碌圆点,Ctrl+L 唤起);**SysMonitor 从 TerminalTabs 移除**,监控小字并入 StatusBar 右侧(`CPU xx% · MEM x.xG/y.yG · ↑/↓网速`);设置弹窗移除"AI 面板宽度%"与"显示 AI 面板",保留会话栏宽度;**文件浏览器**(`fs.rs` 新 + `FileBrowser.svelte`):`list_dir`(经 exec `cd <path> && pwd && ls -la --time-style=long-iso`,`shell_quote` 用 `$(printf %q '...')` 转义防注入;`parse_ls_line` 用 `split_whitespace` 解析,首个字段校验 perms 首字符 `-d/l`),core.rs 增 `fs_cwd: Mutex<HashMap<String,String>>` 记每会话 cwd;`current_dir/mkdir/rename/remove/upload/download` 命令;`remove` 有 `assert_removable`(根/空路径拒绝)+ `check_danger` 双保险;上传 64KB base64 分块追加写,下载 `base64 -w0` 读取;前端:路径条、列表(图标/大小/权限/时间)、双击进目录、右键菜单(打开/重命名/下载/新建目录/删除)、新建/重命名对话框、上传文件选择器(**已完成**,`cargo build`+`cargo test` 28 项(原 23+5 新增)+`npm run build` 通过;CDP 验证:两区 rect 会话栏 264/主列 936(终端 586+文件 180)/状态栏 34/AI 气泡右下角,无聊天栏/无监控条;气泡开窗 340x460、拖拽 +80/+60 生效、状态栏监控小字经假事件渲染、文件面板折叠 32px/展开 180px)
- [x] **P18 反馈修复(布局/悬浮/文件/设置)**:按用户 8 条反馈逐一落地(**已完成**,`cargo build`+`cargo test` 28 项+`npm run build` 通过;CDP 验证):
  1. **AI 悬浮球可拖拽 + 图标重设计**:`.ai-bubble` 由 `right/bottom` 固定改为 `left/top` 由 `bubblePos` $state 控制(初始右下 `window.innerWidth-66, innerHeight-110`),`startBubbleDrag`(mousedown 记偏移)+ 复用 `onMouseMove/onMouseUp`(加 `bubbleDragging` 分支,位置夹取在窗口内);mousemove/mouseup 挂载条件从 `open` 改为 `open || bubbleDragging`(拖拽时也要挂监听)。图标弃用 `🤖` emoji,改内联 SVG(圆角对话框形 + 白色圆点,`currentColor` 随主题)。拖拽不触发 toggle(仅 mousedown 记偏移,click 才开窗)。
  2. **文件面板跟随终端 cd**:FileBrowser 订阅 `onTerminalOutput`,仅对 `p.name===activeTab` 解码文本,按行正则 `/(?:^|#|\$|>|;)\s*cd(?:\s+([^\s;|&]+))?/` 匹配 cd 命令;`cd -` 跳过;`resolvePath` 处理 `~`/`~/`/相对路径(拼 cwd);匹配到即 `refresh(target)`。**exec 通道不共享交互 shell cwd,无法轮询 pwd 实现,只能解析终端输出(best-effort,程序输出里的 `cd xxx` 可能误触发,已验证 `grep foo /etc/passwd`/`not a cd here` 不会误判)**。
  3. **打开文件预览**:`fs.rs` 新增 `read_file(ssh,name,path,limit)` → exec `head -c N` 读取,含 `\0` 判二进制返回"无法预览";超限用 `head -c N+1 | wc -c > N` 探测截断;`FsResult` 加 `truncated: Option<bool>`(serde skip_serializing_if)。core.rs 新增 `fs_read_file` command + main.rs 注册;api.ts 加 `fsReadFile`。前端双击文件/右键"打开(预览)"→ `openFile` → 预览 modal(`.pv-veil`+`.pv-modal`,`<pre>` 等宽 + 关闭/下载、超限提示条)。**手滑点右键选择器时 `:last-child` 指向清空按钮导致 CDP 异常,属测试误操作非代码问题**。
  4. **性能模块回终端上方薄条**:重建 `SysMonitor.svelte`(薄条 ~23px,`--term-bg` 底 + `border-bottom`,CPU/MEM 迷你进度条+负载三值+网速,ok=false 显示 `—`+错误),插回 TerminalTabs 的 `.tabbar` 与 `.term-area` 之间;`StatusBar.svelte` 移除 sysmon 订阅与监控小字(含 `onMount`/`api` import 清理)。
  5. **会话栏折叠大展开按钮**:折叠态(44px)header 改竖向、`flex:1` 占满、隐藏 title/＋,只留 `.expand-btn`(`▶` font-size 1.4rem、accent 色、hover 高亮)作大点击目标;展开态恢复原折叠按钮 `◂`。
  6. **设置去环境变量名输入**:SettingsModal 删除"API Key 环境变量名(回退)"输入框与 `apiKeyEnv` state,提交固定 `api_key_env: "API_KEY"`;Rust 回退逻辑不变(config 字段仍可透传)。
  7. **系统提示词改名**:标签 `系统提示词` → `自定义提示词`(value 绑定不变)。
  8. **下载入口补强**:文件面板底部加 `⬇ 下载` 按钮(紧邻上传,disabled 无选中文件),`onclick`/右键点击文件即记录 `lastFile`(`selectEntry`),下载按钮作用于 lastFile;行点击加 `selected` 高亮。
- [x] **P19 反馈三修复(前端为主+Rust 小修,已完成,`npm run build`+`cargo test` 29 项+CDP 真机验证通过)**:
  1. **文件面板跟随终端 cd(重写为输入层拦截)**:废弃 P18 的"解析终端输出回显"方案(有误判风险),改在 **TerminalTabs 的 `onData` 输入层**拦截:每会话 `inputBufs: Map<string,string>` 累积输入,Enter 时整行匹配 `^cd(?:\s+(\S+))?$` → 回调 `onCd(name,target)`(`cd` 不带参归为 home);转义序列/`\x1b` 清缓冲、Backspace 删尾、Ctrl+C/U 清空;`cd -` 跳过。App 持 `cdReq={name,target,seq}` + `cdSeq` 计数,`handleCd` 递增 seq 触发 FileBrowser;FileBrowser `$effect` 监听 `cdReq`(命中 activeTab 才处理),`resolvePath` 把 `~`/`~/`/相对路径拼成绝对路径后 `refresh(target)`。**FileBrowser 删除 `handleTerminalOutput`/`onTerminalOutput` 订阅(保留 onConnection)**。**注:该输入层拦截方案已被 P23 移除,自 P24 起权威 PWD = OSC7 标记,此处仅存档**。
  2. **AI 悬浮球拖拽后不再误开窗**:`AiFloat.svelte` 加 `bubbleStart`/`bubbleMoved` 状态;`startBubbleDrag`(mousedown)复位 `bubbleMoved=false`;`onMouseMove` 在 `bubbleDragging` 分支位移 >5px 时置 `bubbleMoved=true`;`toggle()`(click)若 `bubbleMoved` 则只复位并 return,吞掉拖拽后的 click。
  3. **文件预览可编辑保存 + 整行选中**:FileBrowser preview 模态从 `<pre>` 改 `<textarea bind:value>`(可编辑),加 `.pv-save`「保存」按钮(复用 `fsUpload(name,path,b64,append=false)` 覆盖写,`TextEncoder→String.fromCharCode→btoa`)+「已保存」提示;truncated 时禁用保存;`openFile` 同时置 `lastFile`(整行即选中)。关闭按钮 `.pv-close`。
  4. **Rust 小修(fs.rs)**:a) `current_dir` 改为**优先返回 cwd_map 已跟踪目录**(exec 通道每次新建,pwd 恒为 home,此前 refresh 后 fs-cwd 永远显示 home 而非 cd 目标);b) 新增 `shell_cd_target(norm)` 处理 `~`/`~/`/`~user`(保持首字符不引号让 bash 波浪号展开,其余部分继续 `shell_quote` 防注入),list_dir/mkdir/rename/remove/upload/download/read_file 全部换用。**坑:直接 `shell_quote("~")` 会生成 `$(printf %q '~')` → bash 不展开波浪号 → `cd: ~: 没有那个文件或目录`**。
  5. **CDP 真机验证**(192.168.79.150 已恢复在线):`cd /opt`→fsCwd /opt 2行、`cd /etc`→fsCwd /etc 179行、`cd ~`→fsCwd /root 21行;`cd /opt && cd` 复合命令不误触(输入层只匹配整行);气泡拖拽(center 坐标 mousedown→mousemove>5px→mouseup,事件间隔须 >300ms 等 $effect flush)气泡移动且不开窗、拖后单击正常开窗;预览 textarea 编辑→保存→远端文件内容确认已改写→「已保存」提示→关闭。
- [x] **P20 文件面板点击同步终端 + 修复 is_dir 字段名 bug(纯前端,已完成,`npm run build`+`cargo test` 29 项+CDP 真机验证通过)**:
  1. **面板 → 终端同步 cd**:FileBrowser 双击文件夹(openDir)/`↑`上一级(goUp)时,除 `refresh` 面板外,再 `api.sendInput(name, \`cd ${shq(path)}\r\`)` 把 cd 命令字节直通后端会话写 PTY(远端 shell 执行并回声,终端可见 `cd '<path>'` 且提示符切换)。`shq = (p) => "'" + p.replace(/'/g, \"'\\\\''\") + "'"`(单引号包裹+转义,防空格/引号目录名注入)。**必须走 `sendInput` 直通后端,不能走 `cdReq`**(那是"用户键盘 onData→handleInput"路径,会二次触发 FileBrowser $effect refresh 造成循环);`sendInput` 不经 xterm onData,不污染 `inputBufs`。删除目录/上传/下载等文件操作不联动终端,仅"进入/回退目录"两个动作联动。
  2. **修复 is_dir 字段名 bug(根因)**:Rust `FileEntry` 带 `#[serde(rename_all="camelCase")]` → JSON 字段是 **`isDir`**,但前端 TS 接口与全部组件一直用 `e.is_dir`(永远 undefined)。**后果:目录行 `e.is_dir ? openDir : openFile` 恒走 openFile → 双击文件夹会错误打开"预览"模态而无法进目录**(P17 起一直隐覆存在,未暴露因为此前双击验证都拿文件/或 P19 用终端 cd 代替)。修复:api.ts `FileEntry.is_dir` → `isDir`,FileBrowser 全部 `is_dir` → `isDir`。验证:双击 `tmp` → 面板 fsCwd /tmp + 终端 `cd '/tmp'` + 提示符 `[root@master tmp]#` + **无预览模态**;`↑` 上一级 → 终端 `cd '/'` → `[root@master /]#` + 面板 `/`。
  3. **坑:PowerShell `(Get-Content -Raw) -replace | Set-Content` 会以 ANSI(非 UTF-8)重写 svelte 文件**,导致所有中文/emoji(如 `—`/`📁`/`⬆`/`⟳`)变 `�?` 乱码。**grep/diff 会显示乱码但内容已永久损坏**。本作踩坑后文件整篇重写恢复(从 dist 编译产物核对原字符串)。**教训:改 UTF-8 文本文件一律用 Edit/Write 工具,勿用 PowerShell 管道替换**。
- [x] **P21 全面回归冒烟(已完成,CDP 真机 192.168.79.150 全项目过一遍)**:
  1. **布局/主题**:三栏 rect(会话 264/终端 936/文件 180/状态 34)正确;默认深色。设置弹窗 tab AI/界面/取消/保存 正常;主题 select 有 light/dark/系统 三项且值可改;**保存受 AI API Key 校验保护**(未配置 key 时保存被拒并显示"环境变量 API_KEY 未配置",属预期)。
  2. **会话链路**:连接 → 圆点 ok + 活动 tab + 终端 "Last login" + 提示符;交互输入 `hostname -I`→192.168.79.150、`pwd`→/etc 均正常回显。会话栏右键菜单(连接/断开/编辑/删除)弹出+关闭正常;会话栏折叠 44px(终端 x 44)与展开均正常。
  3. **面板⇄终端双向联动(回归 P19+P20)**:终端 `cd /opt`→面板 /opt;面板滚到 `/etc/alternatives` 双击→面板 /etc/alternatives + 终端 `cd '/etc/alternatives'` + 提示符 `[root@master alternatives]#`(无预览模态);`↑` 上一级→面板 /etc + 终端 `cd '/etc'`。`cd /usr/` 幽灵仅出现在 cdp-type 单字符连续 dispatch 与面板刷新并发时,重试 `pwd` 干净无幽灵 → **判定为测试脚本噪声非应用 bug**(应用代码仅产生带引号 `cd '...'` 或 xterm 原文)。
  4. **文件**:双击 yum.conf → 预览模态 textarea 可编辑内容;`.pv-close` 关闭;selectEntry 后底部 `⬇ 下载` 按钮启用且点击无报错。
  5. **SysMonitor 薄条**:CPU/MEM/负载/网速(↑5K/s ↓397B/s)实时渲染,2s tick 正常。
  6. **AI 悬浮窗**:气泡(340x460 窗)开合正常;QA 发消息"hello, tell me 1+1"→ 用户气泡 + 后端 error 气泡("AI 未配置,请检查 config.yaml…")(会话无 API key,错误分支即正常);按钮序 QA/Agent/🗑/×/发送,index 3 关闭(回归 P19)。
  7. **搜索/增强**:Ctrl+F(Ctrl 修饰 dispatchKeyEvent)调出搜索条,输入 "etc" 过滤可用;Esc 关闭。

- [x] **P22 刷新竞态修复(文件面板偶发只显示 / 与 /root)**:
  1. **现象**:用户在终端敲 `cd /etc`、`cd /opt`、`cd /tmp` 后,文件面板不跟随,只显示根目录 `/` 与 `/root`;但面板双击进目录与 `↑` 上一级正常。
  2. **根因(竞态)**:FileBrowser 有两个 `$effect` 并发调 `refresh()`——`activeTab` 变化(vs 连接/断开)→ `refresh()`(home/清空);`cdReq` 变化(App `handleCd` 收到终端输入层拦截的 `cd` 回车)→ `refresh(target)`。且 `refresh()` 内部是**两步 invoke**(`fsListDir` 后接 `fsCurrentDir`),两个并发 refresh 的两次 invoke 会交错:慢的 home 刷新(或其中间态)晚到会把 `entries`/`cwd` 覆盖回 `/root`(或 breadcrumb 与列表不一致),表现为"面板停在 / 或 /root、cd 不跟"。缩窄窗口后 CDP 高速连续 `cd` 已可稳定触发。修复前的 P19/P20 方法(输入层拦截 + cdp 单步模拟)时序恰好总赢,故 P21 冒烟未暴露。
  3. **修复(last-write-wins 刷新序列号)**:`refresh()` 开头 `const seq = ++refreshSeq;`,每次 `await`(fsListDir/fsCurrentDir)之后、写 `entries`/`cwd` 之前校验 `if (seq !== refreshSeq) return;`,`finally` 里仅 `seq === refreshSeq` 才 `loading = false`。过期刷新(seq 非最新)直接丢弃结果,保证面板始终停在**最后一次**发起的导航目标,杜绝交错覆盖。
  4. **验证**(CDP 真机 192.168.79.150,`cargo test` 29 项 + `npm run build` 通过):连接→ `/root` 21 行;高速连发 `cd /etc`→`cd /opt` 结果 `/opt` 2 行;断开(右键菜单)→ 面板显示"未连接会话"占位 → 重连后**零等待**紧发 `cd /etc`→`cd /tmp` 结果 `/tmp` 19 行(此前该零等待场景是竞态重灾区);`/tmp` 下 dblclick 进子目录正常。另:**重连后 terminal 焦点/缓冲区会因整页 reload 丢失,先清 `.xterm-helper-textarea` 残余再打字,否则 `cd` 不进 onData**。

- [x] **P23 cd 跟随全面修复(根因修正 + 输出回显权威路径)**:**注:已被 P24(OSC7 权威 PWD 标记)取代,此处仅存档演进史,勿按此实现排错**
  1. **根因修正**:P22 的"刷新竞态"不是主因。真因是 P19 起采用的**输入层拦截**(TerminalTabs `handleInput` 从 xterm `onData` 解析 `cd`)只对手打完整命令有效,其余三种真实输入**必然漏检**:a) Tab 补全(`cd /et<Tab>`)——`\t`(0x09)在 `ch >= " "` 分支被丢弃,缓冲残留 `cd /et`,Enter 匹配到不存在的 `/et`;b) `↑` 历史回放——`\x1b[A` 触发 `data.includes("\x1b")` 清空缓冲;c) Ctrl+V 粘贴——`paste()` 直接 `sendActiveInput` 绕过 onData。此前 cdp 单字符模拟恰好走的是唯一能通过的手打路径,故 P19-P22 全部"验证通过"却始终用户失败。
  2. **修复(唯一权威路径 = 输出回显 + 成功确认)**:
     - **移除** `handleInput` 输入层解析(`onData` 仅转发),cd 检测全部移到 `terminal-output` 处理器的 `feedEcho`。
     - `feedEcho`:`TextDecoder` 流式解码 + `stripAnsi`(CSI/OSC/2-byte ESC)进入每会话尾部缓冲;整个缓冲(含无尾换行的**不完整尾段**,这是成功提示符常见的到达形态)按 `[\r\n]+` 切行处理。
     - **登记**:`/\]\s*[#$]\s*cd(?:[ \t]+([^\s;&|]+))?[ \t]*$/` 匹配"提示符锚定"的复合回显行(`]# cd /etc` / `$ cd` / `cd '/dev'` / `cd ..`),记 `pendingCd`(目标的 `-` 置空跳过)。
     - **失败即作废**:`/: cd(:| )|\bNo such file\b|\bnot found\b/` 命中错误行(`-bash: cd: /x: 没有那个文件或目录` 等,中文错误靠 `: cd(` 前缀而非英文关键词)清除 pending。
     - **成功才触发**:紧随的提示符(`…]# ` 或非 root `…]$ ` 且 3s 内)确认后 `fireCd`;**任何其他中间输出使 pending 作废**,杜绝 `echo 'cd /tmp'` 之类输出误触发。
     - Tab 补全由 readline 输出含**已补全目标**的复合行(非输入残 `cd /et`),历史/粘贴同理,四种输入统一覆盖。快路径/粘贴直判全部删除,无双火问题;`lastCdFire` 800ms 去重保留。
  3. **验证**(CDP 真机 192.168.79.150,`npm run build`+`cargo test` 29 项通过):手打 `cd /opt`→面板 `/opt` 2 行;Tab `cd /et<Tab>`→bash 补成 `/etc/`→面板 `/etc` 179 行;粘贴(直接 `send_active_input "cd /tmp"`)→面板 `/tmp` 19 行;`↑` 历史回放走同一回显路径;反例全部不误触:`cd /nonexistent` 报错后停留在原目录、`echo 'cd /tmp'` 输出不触发、`grep root /etc/passwd` 不触发、复合 `cd /etc && pwd` 不跟随(与旧语义一致);连续 `cd /tmp`→`cd ..`→`cd`(home)链路全程跟随。
  4. **坑**:成功提示符行**不带尾换行**——若只按 `\n` 切分处理"完整行",提示符会滞留尾缓冲永不确认(cd 跟随故障点);必须把不完整尾段一并投入扫描。

- [x] **P24 cd 跟随彻底修复(服务器权威 PWD = OSC7 标记,取代回显猜测)**:
  1. **根因**:P23 仍是"猜 cd 目标 + 等提示符确认"。用户报告 `cd /usr/local/`(尾斜杠)后"面板跟了但列表空"——回显目标解析对尾斜杠/相对路径/`..`/引号/别名/符号链接不可靠,传给 fsListDir 的路径畸变 → cd 成功但 ls 异常/列表空。且回显检测对四种输入共用单点,永远拿不到服务器真实 `$PWD`。
  2. **修复(权威路径 = 服务器真实 PWD,bash 每次重画提示符都发)**:
     - **ssh.rs `open_shell` 注入不可见标记**:shell 通道建好后立即经 `cmd_tx` 发送 `stty -echo; export PROMPT_COMMAND='printf "\033]7;helm:%s\a" "$PWD"'; stty echo; clear`。OSC7(`ESC]7;PWD BEL`)在终端不渲染,注入行经 stty -echo 隐藏 + clear 清屏,**用户无感知**(CDP 验证终端缓冲无 stty/PROMPT_COMMAND/7;helm 任何残留)。
     - **TerminalTabs `feedEcho`**:`stripAnsi` **之前**用 `extractOscPwd` 从原始流(保留跨 data 块拼接的未终止 `ESC]` 片段)提取最后一个 `ESC]N;helm:PWD BEL` 标记 → `syncOscPwd`(剥尾斜杠 + `lastPwd` 去重,普通命令 PWD 不变不触发)→ `fireCd`。`hasMarker` 置位后该会话**停用**回显解析;非 bash(无标记)才回退原 pendingCd 逻辑,互为兜底无回归。
  3. **配套**:FileBrowser `resolvePath` 剥尾斜杠(除根);fs.rs `parse_ls` 过滤 `.`/`..`(导航交给 goUp)。
  4. **验证**(CDP 真机 192.168.79.150,`cargo build`+`cargo test` 29 项+`npm run build` 通过):原报告 `cd /usr/local` 与 `cd /usr/local/` 均 → `/usr/local` **15 行非空**(bin/hadoop/hive/jdk...);Tab `cd /usr/share<Tab>`→`/usr/share` 77 行;粘贴→`/usr/local/lib`(真实空目录 0 行正确);复合 `cd /etc/../usr/share && pwd`→真实 PWD `/usr/share` 77 行(**超越旧锚定解析**:只要 PWD 真变了就跟);失败 `cd /definitely/missing`→PWD 不变面板不漂移;`echo 'cd /tmp'` 不误触;dblclick/goUp 引号路径 `cd '/usr/local/hadoop'`→OSC 读真实 PWD 双向同步;`cd`/`cd ~`→`/root` 19 行。↑历史/粘贴与手打同走"真实 PWD"路径天然覆盖。
  5. **坑**:a) OSC 标记字节经 `ChannelMsg::Data` 原样达前端,必须在 `stripAnsi` **前**提取(stripAnsi 会把 OSC 全剥掉);b) bash `$PWD` 恒为绝对路径、无尾斜杠、`~` 已展开,前端仅做防御性剥除;c) 注入覆盖用户已有 `PROMPT_COMMAND`(绝大多数服务器为空,已知取舍);d) 注入依赖 bash,非 bash 会话靠回退逻辑兜底。
- [x] **P25 Agent 自动化引擎 + 终端内嵌副驾驶(智能远程终端:自然语言任务 → AI 自动执行)**:
  1. **方向(用户定)**:AI = "智能远程终端",自然语言任务由 AI 在远端 shell 逐步执行完成。决策:**仅危险命令确认**(`agent_confirm` 默认 false,危险命令仍强制确认);**先强化"只输出命令"文本协议**(不接 function calling)。UI:用户否决悬浮球/右侧抽屉/虚拟标签/底部面板/悬浮去违和全部方案(**P14 底部聊天、P15 三栏、P17 悬浮窗均被历史否决**),改为**终端内嵌**。
  2. **后端**:
     - `config.rs`:`command_timeout_secs: Option<u64>`(Agent 单命令超时,0/None→60)、`system_prompt_agent: Option<String>`(Agent 专用提示词);`default_agent_confirm()` true→false。
     - `ssh.rs`(新):`task_exec_cmd(cwd,cmd)` 组装 `cd '<cwd>'; { <cmd>; } 2>&1; rc=$?; printf '\n###HELM_END###\n###HELM_EXIT###%s\n###HELM_PWD###%s\n' "$rc" "$PWD"`(cwd `'` → `'\''`);`parse_task_output(raw)->(output,exit_code,pwd)`(无标记→原样,-1/"");`TaskExecResult{output,exit_code,pwd}`;`run_task_exec(handle,cmd,cwd,timeout_secs)`(最小超时 1s,读 Data/ExtendedData 累积,**超时即错误**);+5 个纯函数单测。
     - `agent.rs`:`Agent` 存 `system_prompt_default`/`system_prompt_agent`,`new()` 按模式经 `system_prompt_for` 选提示词;`agent_step(task, prev_output, ctx: &str, sink)` 首轮"当前环境：{ctx}\n新任务：…"、后续"当前目录：{ctx}\n这是上一步命令的执行输出…";`command_timeout_secs()` getter;`clear_history`/`set_mode` 按模式重建 system prompt。
     - `core.rs`:`TaskCtx{name,host,user,pwd}`;`ai_submit(input, pwd: Option<String>)` **Agent 模式锁定活动会话**(无则"未连接会话：请先连接一个会话,再发起 Agent 任务"),pwd 来自前端 OSC7 权威 PWD;`run_ai_job` Agent 分支重写:**循环 agent_step → parse_commands → check_danger → task_exec(锁会话 run_task_exec)**,`agent_confirm || level!=Safe` 才 PendingCommand 等 Approve/Reject/Cancel;cwd 随 `###HELM_PWD###` 回传跟踪、退出码判成功/失败回喂 next agent_step、空回复/模型未给命令直接回喂提示;`CommandStep` 增 `output`(截断 2000 回传前端日志);`exec_command` 移除。
  3. **前端**:
     - `api.ts`:`aiSubmit(input, pwd?)`;`AiConfig` 增 `command_timeout_secs`/`system_prompt_agent`;`CommandStep` 增 `output`;新 `AiLogEntry`。
     - `AiFloat.svelte` **删除**,新 `AiCopilot.svelte` 内嵌 AI 状态条(置于 SysMonitor 与 term-area 之间,~28px;QA/Agent 切换 + 状态文案 + 停止/日志/清空/⌥I 任务;PendingCommand 内插 ⚠ badge + 执行/跳过/取消)。
     - `TerminalTabs.svelte`:收全部 AI props;`Alt+I` 弹 `.ai-task-wrap` 任务输入行(Enter 提交/Esc 取消,`$effect` 聚焦);`Alt+L` 开关 `.ai-log-wrap` 全屏日志(`$effect` 滚底);`aiEcho` prop `$effect` 写活动终端;`syncOscPwd` 增 `onPwd(name,pwd)` 上报权威 PWD;handleKey 增 fromInput 守卫(Ctrl 组合在输入框内不劫持)+ Alt 分支;修 onMount teardown `unsub.then(fn=>fn())` 潜在 Promise 调用 bug。
     - `App.svelte`:新增 `pwds` map(OSC7 权威 PWD)、`aiPending`/`aiTaskOpen`/`logOpen`/`aiLog`/`aiEcho` 状态;`onAi` 全事件处理(streaming 仅 QA 首 chunk 前缀 `[AI]` 并 `qaStream` 去重、commandStep 回显 `[AI 执行/失败]` ANSI 色 + 写日志、done/error 绿/红回显 + 日志、pendingCommand 设 aiPending、busy 复位);`submitTask(name,text)` 带 `pwds[name]` 调 `api.aiSubmit`;`AiFloat` 移除。`SettingsModal.svelte`:命令超时(秒,0 默认)输入框,全部命令确认勾选说明"危险命令始终确认"。
  4. **验证**(`cargo build`+`cargo test` 34 项+`npm run build` 通过;CDP 真机 192.168.79.150 管线全通):AI 状态条渲染于 term-pane(rect 264,59 936x28)且**无悬浮球**;连接后 ⌥I 弹聚焦任务输入行(状态文案同步);QA/Agent 提交流程完整(busy→[AI 错误] 回显进终端 `[root@master ~]#` 提示符后 + `.ai-log-body` 日志条目 + 状态条复位);模式切换 bar `mode-btn.on` + 后端 `ai_mode` 同步;假 PendingCommand 事件注入 → 状态条 ⚠ 警告 + 执行/跳过/取消 三键,点执行清空;⌥L 日志开合。**注:真实多步任务(故意失败纠错 + 真实危险命令确认)未跑——本机无 DEEPSEEK_API_KEY,经用户确认管线已够跳过**。
  5. **坑**:a) **Svelte 5 prop 名必须逐一核对**:App 传 TerminalTabs 用过 `{openTask}`/`{closeTask}`/`{toggleLog}`/`{cancelAi}`(shorthand 名与 Props 的 `onOpenTask`/`onCloseTask`/`onToggleLog`/`onCancel` 不符 → 运行时 `onOpenTask is not a function`),TerminalTabs 传 AiCopilot 同样 `{aiMode}` vs `mode` 不符 → 按钮永不 `.on`。props 全走显式 `prop={value}` 最稳。b) **vite HMR 全量 reload 会清空前端会话状态**:编辑源文件后页面重载,tab/终端缓冲全丢,后端会话仍 Connected 但不发事件 → 必须 `disconnect_session`+`connect_session` 重建前端 tab;期间 `.ai-bar`/`.tab` 短暂为 0 属正常(dev-log 里 teardown 期 `unsub is not a function` 异常是**旧模块**卸载噪音,已修新代码为 `unsub.then(fn=>fn())`)。c) **无 API key 时 Agent::new 失败 → CoreState 回退 Agent::default()**,chat/agent_step 走 `ensure_ready` 报 "AI 未配置…" → 整条 busy→error 回显链路就是验证手段。d) CDP 注入假事件用 `window.__TAURI_INTERNALS__.invoke('plugin:event|emit',{event:'ai',payload:{kind:'pendingCommand',...}})`(沿用 P22 方法)。
- [x] **P26 工程化迭代(细粒度锁 + 事件驱动输出 + SFTP 直传 + 窗口持久化 + 会话密码加密 + 主机密钥 TOFU,已完成,`cargo build`+`cargo test` 45 项+`npm run build` 通过;live 测试因服务器离线自动跳过,真机冒烟待服务器恢复补跑)**:
  1. **P26-1 零风险小修**:core.rs 会话状态读取收敛(一次拿锁读多值);TerminalTabs.svelte 死代码清理;App.svelte Promise.all 合并。
  2. **P26-2 会话密码 DPAPI 加密**:crypto.rs 密文加 `enc:` 前缀;core.rs add/update_session 落盘前加密、列表回传哨兵 `SECRET_MASK("·")`;ssh.rs connect 先解密再认证(**旧版明文 config 原样兼容**);SessionForm.svelte placeholder"已保存(留空则不修改)"。
  3. **P26-3 主机密钥 TOFU**:新 known_hosts.rs `KnownHostsStore`(~/.config/helm/known_hosts.yaml,`host base64(key)` 逐行);SshHandler 实现 `ServerCheck` + `key_error` 捕获首次/变更 → 连接失败提示"主机密钥与首次记录不同…"(防止中间人);`forget_host_key`/`host_key_fingerprint` commands 供前端展示/重置。
  4. **P26-4a exec 输出上限**:`MAX_EXEC_OUTPUT`(8MiB,run_exec 超限即停止累积防大输出拖垮内存)+ `MAX_TASK_OUTPUT_RETAIN`(64KiB,agent 任务输出 `trim_to_tail` 只保留尾部回喂模型)+ 单测。
  5. **P26-4b/4c 细粒度锁 + Notify 事件驱动输出**:
     - ssh.rs:`SshManager` 改 `{ sessions: Arc<Mutex<HashMap<String, Arc<Mutex<SshSession>>>>> }` + `active: Arc<Mutex<Option<String>>>` + `notify: Arc<Notify>`;**map 锁只做增删/查找,瞬时持有,网络操作(connect/读通道/写 PTY/exec)拿到 Arc handle 后锁外执行**,消灭"连接一个会话卡死其余会话"的全局锁问题;全部方法改 `&self` + async。
     - `open_shell` 读任务在 Data/ExtendedData/通道关闭处 `notify.notify_one()`;新增 `wait_output()`(drain 后等下一次通知,兼顾 Notify 只存一个许可的语义)。
     - core.rs:`CoreState.ssh: Arc<SshManager>`(**去掉外层 Mutex**),全部 command 适配 async &self;`spawn_output_poller` 从 50ms 轮询重写为 **Notify 事件驱动**(无输出即休眠,唤醒后全量 drain + 断线检测发 `connection` Disconnected),并加 **500ms 心跳兜底**:通道刚结束瞬间 `is_closed()` 可能仍为 false,心跳补检避免远端断开/用户 `exit` 的状态永久滞留(原轮询天然覆盖,事件驱动需显式兜底)。
     - fs.rs / monitor.rs 签名同步改 `&Arc<SshManager>` + `.await`。
  6. **P26-5 SFTP 协议重写文件传输**:Cargo.toml 加 `russh-sftp = "2.4.0"`(连带锁入 dashmap v6.2.1、gloo-timers v0.4.0、hashbrown 0.14.5、serde_bytes v0.11.19,chrono 亦被编译);ssh.rs `sftp_handle()` 懒建缓存(subsystem 通道 → `SftpSession::new(channel.into_stream())` → `set_timeout(60)` → 存 `SshSession.sftp`);fs.rs 全部命令改 SFTP:list_dir(canonicalize+read_dir)/current_dir(cwd_map 优先)/mkdir(递归)/rename/remove(danger 拦截 + `Box::pin` 递归删除)/upload(base64 解码 + `open_with_flags` 截断覆盖或 seek 追加)/download(`sftp.read` → base64)/read_file(预览 + NUL 判二进制 + truncated);删除 run_cmd/shell_quote/parse_ls_line/get_cwd/read_dir 等 exec 辅助;时间格式化用**零依赖 civil_from_days**(Howard Hinnant)转 UTC;**前端 api.ts/组件零改动**(upload/download 继续复用 FsResult.message 承载 base64)。测试:新增 `sftp_live_readonly`(离线自动跳过),纯函数测试改验证新实现。
  7. **P26-6 窗口尺寸/位置持久化(零依赖)**:config.rs UiConfig 加 `window_x/window_y: Option<i32>`(`#[serde(default, skip_serializing_if="Option::is_none")]`);main.rs setup 恢复尺寸(LogicalSize)+ 位置(两者均 Some 才 set_position,否则系统默认/居中)、`on_window_event(Moved)` 记录坐标到内存、`RunEvent::Exit` 兜底取当前 `outer_position`/`outer_size`(转逻辑尺寸)统一落盘;`update_ui_config` 保留已有坐标(前端设置弹窗不携带位置字段),并同步 set_position。
  8. **验证**:`cargo build` + `cargo test` **45 项全过**(44 + sftp_live_readonly;3 个 live SSH + 1 个 live SFTP 因 192.168.79.150 离线自动 skip 非失败)+ `npm run build` 通过。**待办:服务器恢复后补真机冒烟(SFTP 列表/上传/下载/预览、窗口位置记忆、Agent 任务)。**
  9. **坑**:a) `win.scale_factor()` 返回 `Result<f64, Error>`,须 `unwrap_or(1.0)`;`outer_size().to_logical::<f64>(sf)` 参数是 f64 非 Result。b) russh 0.44.1:`Channel::into_stream()` + `request_subsystem`;russh_sftp::protocol 的 FilePermissions 是 bitflags(含 Display)、FileType 有 is_dir/is_file/is_symlink、OpenFlags。c) 时间戳 1786791845 = 2026-08-15 11:04:05 UTC。d) **live 测试跳过路径依赖 config 查找失败**(测试 CWD 下 `load_config(None)` 不查 `../config.yaml`,找不到会话定义 → 直接 skip),属预期。e) 心跳兜底是事件驱动 poller 的必要补强,勿删。
- [x] **P27 a11y 清理 + 档案修正(已完成,`npm run build` 通过,a11y 警告清零;cargo 侧无改动)**:
  1. **AGENTS.md 目录树修正**:档案 §1 原写组件位于 `frontend/src/lib/components/`,实际为 `frontend/src/components/`,树形图已改正。
  2. **交互元素键盘可达**:SessionPanel 会话行(`ul/li` → `div.session-item`,加 `role="button"` + `tabindex="0"` + Enter/空格=选中)、TerminalTabs 标签页(同三件套)、FileBrowser 文件行(`role="button"` + Enter=打开/空格=选中);三处补 `:focus-visible` 高亮(2px accent outline)。
  3. **遮罩/容器类静默**:`.veil/.ctx-veil/.pv-veil/.pv-modal/.fs-table/.fs-resize` 属点击捕获/拖拽手柄等无独立语义元素,加注释 + `svelte-ignore`(右键容器级 preventDefault 是行级 openContext 之外的空区域兜底,保留)。
  4. **SessionForm label 关联**:6 组 `<label>文本</label><input/>` 改为包裹式 `<label>文本<input/></label>`(隐式关联,消除 6 条 `a11y_label_has_associated_control`,与 SettingsModal 既有写法一致)。
  5. **保留项(非 a11y)**:`state_referenced_locally` 27 条(SessionForm 6 + SettingsModal 21)为设计意图——两个弹窗每次打开经 `{#if}` 条件重挂载,`$state(prop.x)` 快照语义正确;Svelte 无表达式级豁免(改 `$derived` 会破坏快照、逐条 ignore 增噪),判断保留不处理。
  6. **坑(a11y 规则,详见 §7 P27)**:
     - **Svelte 5(runes 模式)`svelte-ignore` 多码必须用逗号分隔**:`<!-- svelte-ignore code1, code2 -->`;解析器遇第一个无逗号码即 break——`code1 code2` 只压第一个(实测 `a11y_click_events_have_key_events` 被压、`a11y_no_static_element_interactions` 仍报)。
     - **`<li role="button">` 非法**:报 `a11y_no_noninteractive_element_to_interactive_role`;交互行须用 div(或真 `<button>`)。
     - **div 交互元素三件套** `role="button"` + `tabindex="0"` + `onkeydown`(Enter/空格)可一次满足 `a11y_click_events_have_key_events` / `a11y_no_static_element_interactions` / `a11y_no_noninteractive_element_interactions` 三条规则。
- [x] **P28 服务器恢复真机冒烟收尾 + connect 超时修复(已完成,`cargo build`+`cargo test` 46 项+`npm run build`+`cargo tauri build` 通过)**:
  1. **根因:connect 超时 15s → 45s**:服务器 192.168.79.150 恢复在线后,port 22 通但连接必失败——临时诊断测试 `tmp_diag_connect_phase` 定位:TCP/KEX 约 20ms 通过,`authenticate_password` 卡 **~20s** 才成功(OpenSSH 7.4 + 疑似 UseDNS/反向解析慢)。15s 超时必然误报"不可达";放宽到 **45s** 后首次连接即成功。错误文案同步 `超时(45秒)`。(临时诊断测试已删除)
  2. **新增 `sftp_live_readwrite` live 测试**(fs.rs):mkdir(幂等)→ upload(覆盖+追加)→ list_dir → read_file(完整/截断 truncated/二进制 NUL 拒绝)→ download(base64 回读)→ rename → remove(递归)→ 断言行消失;单跑 20.26s。测试数变为 **46**(41 无网络 + 3 个 SSH live + sftp_live_readonly + sftp_live_readwrite)。
  3. **离屏哨兵守卫(P26-6 补强)**:实测最小化时 Windows 把窗口移到 (-32000,-32000),落盘后下次启动恢复不到屏幕内且 config 同步脏值。4 处加 ±20000 阈值守卫:main.rs `on_window_event(Moved)`(abs<20000 才记录)、`RunEvent::Exit` 兜底(同守卫)、setup 恢复(任一坐标 abs≥20000 → `set_position((0,0))`)、core.rs `update_ui_config`(abs<20000 才 set_position)。
  4. **release 重打包**:移除 main.rs 全部临时 DBG eprintln;`cargo tauri build` 出新 exe(含 45s 超时 + 守卫),msi/nsis 同步更新。**release 冒烟全通**:启动窗口 1214x838 title 'Helm';移动 (300,180) 逻辑 → WM_CLOSE → config 落盘 `window_x: 375 / window_y: 225`(**物理像素 @125% DPI**),重启即恢复该位置;测试副本清理后 `cargo test` 46 项全过(35.8s,含全 live)。
  5. **坑**:
     - a) **live 测试在 `F:\Helm\src-tauri` 目录跑**:测试 CWD 下 `load_config(None)` 查不到 `./config.yaml` → 直接 skip;要真跑 live 须先把根 `config.yaml` **复制为 `src-tauri/config.yaml` 临时副本**,跑完删除。
     - b) **P26-6 落盘坐标是物理像素**:移动逻辑 (300,180) @ DPI 125% 落盘为物理 (375,225);恢复时 `PhysicalPosition::new(x,y)` 语义一致,无需换算,但排查 config 时别把 375 当逻辑值。
     - c) **`FindWindowW($null,'Helm')` 在本机会间歇性返回 Zero**:可靠的定位法是用 Add-Type 的 `EnumWindows` + `GetWindowThreadProcessId` 筛 PID + `IsWindowVisible` + 取面积最大者;**C# lambda 不能捕获 `ref/out` 参数**(找窗函数跨方法返回句柄用静态字段 `Best`,勿用 `out` 参数传给 lambda)。
     - d) **应用自身能启动、本地网络正常也"显示断网"的另一可能**:旧 release exe(15s 超时)连慢认证服务器必失败;先查用户双击的是否旧构建,重建 release 即可。
- [x] **P29 代码审查三修复(快捷键回归 / 危险命令正则绕过 / Agent 停止失灵,已完成,`cargo test` 56 项+`npm run build` 通过)**:
  1. **修复 TerminalTabs `handleKey` fromInput 误伤 xterm textarea(前端回归)**:xterm 输入是隐藏 `<textarea class="xterm-helper-textarea">`(位于 `.xterm` 容器内),旧 `fromInput = INPUT||TEXTAREA` 在**终端聚焦时恒为 true**,导致 P25 之后 Ctrl+F(搜索)、Ctrl+C(有选区复制)、Ctrl+V(粘贴)全部静默失效(选自 Ctrl+C 直接变 SIGINT)。修复:加 `const inTerminal = !!t?.closest?.(".xterm")`,`fromAppField = (INPUT||TEXTAREA) && !inTerminal`,三处 `if (fromInput)` 改 `if (fromAppField)`(TerminalTabs.svelte `handleKey`)。
  2. **safety.rs 从"子串正则"重写为"shell 分词 + 语义判级"**:新增 `split_commands`(按 `;`/换行/`&&`/`||`/`|` 引号感知切命令段)+ `tokenize_segment`(引号感知分词);`check_danger` 先 Critical 扫描全部段、再 Warning 扫描。规则:`rm`+递归+强制+`/` 前缀目标 → Critical(`rm -r -f /`/`rm -rf -- /`/`rm -rfv /`/`sudo rm -rf /`/`rm -rf / && reboot` 全覆盖,修掉旧 `rm\s+-rf\s+/` 的变体绕过);`rm -rf` 非根 → Warning;`chmod`+含 777+`/` 前缀 → Critical;`shutdown/reboot/pkill/killall/fdisk/parted` 改用 `first_command`(跳过 `sudo/env/time/nohup/nice/setsid/stdbuf` 包装器与前导选项,**`echo shutdown`/`echo pkill` 不再误报**,而旧子串正则误报);`mkfs*`/`dd` `of=/dev/sd|nvme|mapper|vd|hd*` → Critical;fork 炸弹优质正则 `:\s*\(\s*\)\s*\{`(覆盖 `:(){` 与 `: ( ) {`)。`echo 'rm -rf /'`(引号整体参数)→ Safe。+10 条单测(共 56)。
  3. **Agent 步骤 select 监听 ctl_rx,drop 即中断 HTTP(core.rs run_ai_job)**:旧 `ag.agent_step(...).await` 在**持 `agent` 互斥锁期间做完整模型调用**(最长 timeout_secs),期间 `ctl_rx` 无人 poll,「停止」要等 step 返回(最坏 60s),`ai_stop` 的 reset_task/clear_history 也因等锁被卡。修复:仿 QA 分支,`tokio::pin!(fut)` + `tokio::select!` 同时 poll `&mut fut` 与 `ctl_rx.recv()`;收到 Cancel → emit `Done{已停止}` + `finished=true`,**块尾 drop 未完成的 future(中断 HTTP)+ 释放 agent 锁**。
   4. **坑点与教训**:a) rm 短选项组合 `-rf` 旗标判定**必须两个独立 if**,写成 if/else-if 链只识别出 r(r 分支命中后 else-if 短路,f 永不赋值)→ `-rf` 判不出 force → `rm -rf /` 竟返回 Safe(测试直接抓到);b) fork 炸弹空格变体 `: ( ) {` 需 `:\s*\(\s*\)\s*\{`,旧式 `:\(\(\s*\)...\s*\{`(冒号后无空白)匹配不到;c) `split_commands` 会把 fork 炸弹的 `:|:&` 切成多段,靠首段匹配兜底;d) `echo shutdown`/`echo pkill`:子串正则必误报,命令名判级须用 `first_command`(跳过包装器与 `-` 选项);e) select 取消分支里 `ctl_rx.recv()` 返回 None/陈旧控制一律 `finished=true` 保守停(真实环境仅停止会发控制)。
- [x] **P30 代码审查二轮修补(agent.rs 两处配置/流式健壮性,已完成,`cargo test` 58 项+`cargo build`+`npm run build` 通过;live 测试本机服务器在线全过)**:
  1. **extra_body 不可覆盖核心字段(agent.rs request_body)**:旧 `body[k] = v.clone()` 会整体覆盖本函数已组装的 `model/messages/stream/temperature/max_tokens`,用户误配 `extra_body: {"stream": false}` 会静默破坏流式。新增常量 `RESERVED_BODY_KEYS`(上述 5 键),合并时跳过;非保留键(如 `response_format`)照常透传。+单测 `extra_body_cannot_override_core_fields`。
  2. **流内 error JSON 不再静默丢弃(agent.rs)**:部分服务端在 HTTP 200 后以 `data: {"error":{...}}` 中途报错,旧 `parse_sse_line` 因无 `choices` 归为 Ignore 静默吞掉 → 已开始输出时返回**截断的部分文本**、用户无任何提示。`SseEvent` 增 `Error(String)` 分支,`parse_sse_line` 提取 content 前先查 `value["error"]`(提取优先级:error 字符串原样 > `error.message` > 整体序列化);`call_api_stream` 匹配 Error → `return Err`,由 `call_api` 既有 `started` 语义决定回退非流式(未开始)或直接报错(已开始),零新增分支。+单测 `parse_sse_stream_error_object`。
  3. **验证**:全量 `cargo test` **58 项**(56 旧 + 2 新)全过——本机 192.168.79.150 在线,含 3 个 SSH live + 2 个 SFTP live;`cargo build` 零警告;`npm run build` 通过(前端零改动)。
  4. **坑**:error 为字符串形式时 `err.to_string()` 会带 JSON 引号(`"boom"`)→ 断言抓到,改为 `err.as_str()` 优先再取 `message` 字段,最后才序列化兜底。
- [x] **P31 布局重构(结构清理 + 终端为主,已完成,`npm run build` 通过 + CDP 冒烟验证)**:
  1. **A 结构清理**:删除 App.svelte `.body` 冗余层(与 `.main-row` 完全同向同尺寸的 flex row 包装),`.main-row` 直接成为 `main.app` 子级,CSS `.body` 规则一并删除。嵌套从 4 层(.app/.body/.main-row/.main-col)压到 2 层,零视觉变化。CDP 验证 `main.app` 直接子级 = `[main-row, statusbar]`。
  2. **B 终端为主(两处)**:
     - **文件面板默认折叠**:FileBrowser `collapsed` 默认 `false→true`(180px→32px),折叠态 header 显示"📁 文件"标题提示,点击 ▲ 展开;cd 双向联动不受影响($effect 照常 refresh)。
     - **AiCopilot 条件渲染**:TerminalTabs 的 `<AiCopilot>` 包进 `{#if aiBusy || aiPending || aiTaskOpen}`,空闲隐藏(顶部只剩 tabbar + SysMonitor 23px,终端 +28px),busy/危险确认/任务输入中完整出现,功能不减;SysMonitor 按 P18 定案保留在终端上方不动。
     - **StatusBar 模式切换入口**:因 AiCopilot 空闲隐藏,AI 状态文字("AI 问答/Agent")从只读 span 改为可点击 `<button class="mode-link">` 循环切换模式(P31;busy 时 disabled),App 传 `onModeChange={handleModeChange}`。
  3. **验证**(`npm run build` 通过,仅 P27 已知 `state_referenced_locally` 保留警告;CDP 冒烟 @9229):`main.app` 子级=[main-row,statusbar];会话栏 264/主列 936/状态栏 34;文件面板 `collapsed` 高度 32;`.ai-bar` 空闲 null → 注入 `{kind:'busy',busy:true}` 后出现 28px → busy:false 复位后消失;状态栏 mode-link 点击 `问答→Agent` 生效。
  4. **坑**:a) Svelte 条件渲染 `{#if aiBusy || aiPending || aiTaskOpen}` 中 `aiPending` 为 null 时是 falsy,直接当布尔判断即可;b) StatusBar `mode-link` 用 `<button>`(非 span)才能吃 disabled/hover 语义;c) 折叠态文件面板 header 需在 `{#if collapsed}` 分支单独渲染标题(Svelte 模板 `{:else}` 切换)。
- [x] **P32 多平台会话第一期:kind 模型 + Windows SSH 终端(已完成,`cargo build`+`cargo test` 58 项+`npm run build` 通过,CDP 冒烟验证)**。
  1. **模型**:config.rs 新增 `SessionKind` 枚举(lower `linux`/`windows`/`rdp`,serde default,默认 Linux,旧配置无 kind 自动 linux);`SessionInfo` 加 `kind: SessionKind` 字段(`#[serde(default)]`,向后兼容)。**三期规划**:P32 SSH-Windows 终端交互 / P33 RDP 控屏(mstsc)/ P34 Windows 监控+Agent+SFTP。
  2. **后端**:ssh.rs `open_shell` 增 `kind: SessionKind` 参数,**仅 `kind==Linux` 注入 OSC7 PROMPT_COMMAND**(windows/rdp 跳过,否则 cmd/PowerShell 不识 stty/printf 报错污染终端);core.rs `connect_session` 从 config 取 kind 传给 open_shell;live 测试调用点 `open_live` 同步传 `info.kind`。
  3. **前端**:
     - api.ts `SessionKind` 类型 + `SessionInfo.kind?: SessionKind`(可选,兼容旧会话)。
     - SessionForm.svelte:新增"平台"下拉(Linux SSH / Windows SSH / 远程桌面 RDP),`onKindChange` 切换默认端口/用户(linux→22/root,windows→22/Administrator,rdp→3389/空);**rdp 隐藏密码/私钥输入 + 提示"由 Windows 系统提示输入密码"**(mstsc 交互式,避免明文落盘),且 rdp 不要求密码/私钥(校验跳过)。
     - SessionPanel.svelte:会话行显示类型徽标(`badge{kind}`:windows→"Win" 蓝、rdp→"RDP" 紫,linux 不显示)。
     - App.svelte:`kinds` derived map(会话名→kind,默认 linux)传入 TerminalTabs/FileBrowser;**rdp 会话的 selectSession/connectSession 直接 return**(不是 SSH,不建终端标签);linux 行为不变。
     - TerminalTabs.svelte:收 `kinds` prop;`feedEcho` 对 windows 会话**整体跳过 cd 跟踪**(无 OSC7/bash 回显语义,避免 PWD/cd 误触面板);SysMonitor 对 windows 活动会话**隐藏**(采集 /proc 必失败,降级为占位无意义)。
     - FileBrowser.svelte:收 `kinds` prop;windows 会话 `refresh` 置空跳过 + 显示占位"Windows 会话暂不支持文件面板(P34 开放)"。
  4. **验证**(`cargo build`+`cargo test` **58 项全过**+`npm run build` 通过;CDP 冒烟 @9229):会话栏 1 行无徽标(linux 正确);＋ 弹表单,select 三选项正确;切 windows→ port 22 / user Administrator;切 rdp→ port 3389 / user 空 / 密码私钥输入消失 / `.hint` 提示出现(验证须 `new Event('change',{bubbles:true})`,Svelte 5 事件委托要冒泡);取消表单后 `.app`/`.session-item` 布局正常(1200x766)。
  5. **坑**:a) **Svelte 5 事件委托下 `dispatchEvent(new Event('change'))` 必须带 `bubbles:true`**,否则委托监听收不到(表单默认校验魔改值断言全假);b) rdp 会话状态圆点恒为 off(非 SSH 无真实状态,占位),右键菜单"连接/断开"对 rdp 由 App 门控静默拦截;c) `feedEcho` windows 门控**在 stripAnsi/decoder 之前 return**,否则 TCP 分包残留进 decoders/echoBuffs 无意义堆积;d) FileBrowser 占位用独立 `{#if isWindows}...{:else}...{/if}` 包裹整块 body,注意块闭合层级。
- [x] **P33 远程桌面控屏(RDP,mstsc,已完成,`cargo build`+`cargo test` 58 项+`npm run build` 通过,CDP 冒烟验证)**。
  1. **后端**:core.rs 新增同步 command `rdp_connect(name)`(非 async,blocking_lock 读 config):校验会话存在且 `kind==Rdp` → 组装 .rdp 文本(`full address:s:` / `server port:i:` / `prompt for credentials:i:1` / `enablecredsspsupport` / `authentication level:i:2` / `screen mode id:i:1` / `session bpp:i:32` / `username:s:`(user 非空才写))→ 写入 `%TEMP%/helm-rdp-<pid>.rdp` → `mstsc.exe <path>` 后台拉起;main.rs 注册;api.ts 加 `rdpConnect(name)`。
  2. **前端**:SessionPanel 增 `onRdp` prop;右键菜单"连接"与双击对 `kind==rdp` 会话改调 `onRdp`(App `rdpConnect`),SSH 会话不受影响;**密码不写入 .rdp**(系统提示输入,避免明文落盘)。
  3. **验证**(CDP @9229):invoke 注入 `add_session{name:winbox,kind:rdp}` → `list_sessions` 回 `winbox:rdp`、旧会话 `1` 仍 `linux`(serde default 向后兼容);reload 后 UI 徽标 `1[no-badge] | winbox[RDP]`;`rdp_connect` 返回 ok,`%TEMP%/helm-rdp-*.rdp` 内容字段齐全、mstsc 进程拉起(PID 存在);清理测试会话与临时 .rdp。
  4. **坑**:a) CDP 脚本 `Runtime.evaluate` 对 Promise 表达式需 `awaitPromise:true` 才拿得到 resolve 值,否则 value 恒 undefined;b) invoke 直接加会话/删除后 App `sessions` 状态不会自动刷新(加载时机在 mount),断言 UI 前先 `location.reload()`;c) `rdp_connect` 是同步 command 走 blocking_lock 读 config(短暂持锁,可接受),勿改成 async + .lock().await 以免与 Tauri 规范冲突。
- [x] **P34 Windows 监控 + Agent + SFTP(由 P32 三期规划落地前两期核心,已完成 develop 侧代码,`cargo build`+`cargo test` 61 项+`npm run build` 通过;**无 Windows 真机,`cargo tauri build` 与真机冒烟待 Windows 测试机就绪后补跑**)**。
  1. **映射旧功能到 Windows**:
     - **监控(monitor.rs)**:`collect_command_windows()` 单条 PowerShell CIM 采集(CPU `_Total` PercentProcessorTime、内存 TotalVisibleMemorySize/FreePhysicalMemory、网卡 BytesReceivedPersec/SentPersec 求和、负载行写 `0 0 0`),沿用 SEP1-4 分段使 `section()` 复用;新增 `WinSample`/`parse_win_sample`/`build_win_payload`(cpu=瞬时 %、mem=总量/可用、load 空、rx/tx B/s)+ 纯函数单测 `parse_windows_sample`(CPU 23 / 16384MB-8192MB / rx12345 / tx6789)。poller 按 `ssh.kind_of(&name)` 分支取采集命令与解析,`kind_of(name)` 返回会话 kind(未连接默认 Linux)。
     - **Agent 命令执行(ssh.rs,PowerShell 分支)**:新增 `task_exec_cmd_windows(cwd,cmd)`——**经 `powershell -NoProfile -NonInteractive -EncodedCommand` 调用**(脚本 UTF-16LE base64,`encode_powershell()` 用 data_encoding BASE64),不依赖远端默认 shell(cmd/PowerShell 均可用)且避免层层引号转义;脚本 = `Set-Location -LiteralPath '<cwd>'; { cmd; } 2>&1; $rc=$LASTEXITCODE; if($null -eq $rc){$rc=0}` + `###HELM_END###/###HELM_EXIT###$rc/###HELM_PWD###$((Get-Location).Path)` 标记,**标记格式与 Linux 版一致 → `parse_task_output` 复用**(cwd 单引号转义 `''`)。`run_task_exec` 增 `kind: SessionKind` 参数按 kind 分发;core.rs `task_exec` 经 `ssh.kind_of(name).await` 传入。+2 个单测(EncodedCommand 组装+解码校验 / cwd 引号转义)。
     - **SFTP 文件面板(前端 FileBrowser,解锁 Windows)**:
       - `isWindows`/`isBlocked`(rdp 才占位)/`sep`(windows `\` vs `/`)三个 `$derived` + `joinPath(base,name)` 统一拼路径;`refresh` 门控从"windows 拒绝"改为"仅 rdp 拒绝"。
       - **面板 → 终端 cd 同步(`syncTerminal`)仅 Linux 生效**(windows 无 OSC7/bash 回显语义;cmd 与 PowerShell 的 cd 语法/引号不统一,不冒然注入);windows 点目录/↑只刷新面板不写 PTY。
       - `goUp` 增加 windows 分支(`lastIndexOf` 取父层,盘符根 `C:` 上溯到 `\`);`resolvePath` 识别驱动盘绝对路径(`^[a-zA-Z]:[\\/]`)+ 剥尾斜杠兼容 `\`;cdReq 终端→面板联动仅 `kind==="linux"`。
       - 模板占位文案改"RDP 会话暂不支持文件面板";后端 fs.rs 全部 SFTP 命令天然跨平台(SFTP canonicalize 风格化),零改动。
  2. **验证**:`cargo build` 零错误(原 P34-1 遗留两处编译错修复:`connect` 里 `sessions.insert(name,..)` 后 `get_mut(&name)` 借用已移动 → 改 `name.clone()`;`kind_of` 尾部表达式临时 MutexGuard 借用 → 改 `let guard = …; guard.kind` 先绑再取)、`cargo test` **61 项全过**(58 + P34-1 监控 1 + P34-2 Agent 2)、`npm run build` 通过。**待办:Windows 测试机(OpenSSH Server + RDP)就绪后补真机冒烟:PowerShell 监控数字、Agent PowerShell 命令执行与 `$PWD` 回传、文件面板 `C:\` 路径导航/上传/下载/预览保存、rdp 仍占位**。
  3. **坑**:a) `task_exec_cmd_windows` 断言必须先解码 base64(UTF-16LE→String)再 contains,直接在编码串上断言必然失败;b) windows 的 cwd 是 `C:\...` 风格,`Set-Location -LiteralPath` 单引号转义 `''` 是 PowerShell 规则(与 bash `'\''` 不同);c) 前端 `sep`/`joinPath` 需在 `$derived` 之后定义、运行时求值(勿在脚本顶层硬编码 `/`);d) Shell 里 grep 中文路径差一斜杠会正则报错,写路径时注意转义。
- [x] **P35 代码审查修复批(安全/功能 bug,已完成,`cargo build`+`cargo test` 61 项+`npm run build` 通过)**:
  1. **safety.rs 长选项判级失效(安全缺口)**:`rm_flags` 对 `--recursive`/`--force` 只 `strip_prefix('-')` 一次,得到 `-recursive` 再比较 `== "recursive"` 永假 → `rm --recursive --force /` 检测不到 Critical。修复:长选项分支改 `strip_prefix("--")` 优先判定;`critical_rm_variants`/`warning_rm_variants` 补长选项变体断言。
  2. **core.rs 陈旧控制信号(安全相关)**:a) Agent select 的 ctl 分支对 Approve/Reject 也 `finished=true`,一个陈旧确认信号会静默终止任务 → 改 loop+select,仅 Cancel 结束,其余信号忽略继续等模型返回;b) 推理/执行期间滞留 channel 的 Approve 会被下次 `PendingCommand` 的 `recv()` 消费 → **危险命令未经确认即执行**;修复:发 PendingCommand 后先 `try_recv` drain 陈旧信号(遇 Cancel 则结束),再进 `recv().await`。
  3. **config.rs save_config 原子写入**:直接覆盖 config.yaml 写一半崩溃会丢全部会话(含 DPAPI 密文密码);改写 `*.yaml.tmp` + `fs::rename` 原子替换。
  4. **FileBrowser mkdir 拼出 `undefined` 路径(功能 bug)**:`submitName` 用 `joinPath(cwd, pendingName.name)` 而 mkdir 时该字段为 undefined → 创建名为 `undefined` 的目录;修复:mkdir/rename 新路径均用输入框 `name`。
  5. **前端监听泄漏**:`api.onSysMon`/`onConnection` 返回 `Promise<UnlistenFn>`,SysMonitor `onMount` 直接 `return unsub`(Promise 被忽略)、FileBrowser 清理时把 Promise 当函数调用抛 TypeError;统一改 `let un; promise.then(fn=>un=fn); return ()=>un?.()` 模式。
  6. **AI 回显丢字**:App `aiEcho` 单槽状态在密集 streaming 事件同批次只留最后一个对象 → 改队列数组(seq 递增,超 200 裁剪),TerminalTabs `$effect` 按 `lastEchoSeq` 顺序消费逐条写入。
  7. **连接失败可见化**:`Failed{error}` 事件建标签 + 红色 `[连接失败] <error>` 回显进终端(此前只有灰点,失败原因不可见)。
  8. **小修**:delete_session/update_session 改名清理/迁移 `fs_cwd` 键;rdp_connect 拉起 mstsc 后 30s 延迟删除临时 .rdp(避免残留主机信息)。
  9. **审查确认不改(误报)**:SettingsModal 空 API Key——后端 `""|"·"|null → 保留原密文`,非 bug;`next.expect` panic——`if finished { break }` 在 expect 之前,无 panic 路径。**遗留待办(评估过、暂不处理)**:SFTP 下载整文件入内存无上限、known_hosts pending_error 单槽位并发串台、agent.rs 每请求新建 reqwest::Client、ai_busy TOCTOU、Agent 危险检测不覆盖 `$(...)`/xargs 二次执行。
- [x] **P36 README 生成**:项目根新增 `README.md`(中文,面向用户/贡献者):定位与功能特性(终端/多平台/SFTP/监控/AI 副驾驶/安全)、快捷键表、源码构建与测试命令、config.yaml 配置样例与字段说明(建议密钥经界面填写自动 DPAPI 加密)、目录结构、技术栈、MIT 许可。内容以本档案 §1/§2/§4 为准提炼,无代码改动。
- [x] **P37 应用图标重制(已完成,`cargo build` 通过 + exe 图标验证)**:替换 Tauri 默认占位图标为项目专属「船舵」图标(Helm = 舵,"你掌舵,AI 执行")。
  1. **源图生成**:`scripts/gen-icon.mjs`(新,零依赖,Node 内置 zlib 手写 PNG 编码)程序化绘制 1024×1024 RGBA:深色渐变圆角方底(对角 #1E2637→#0B0E14)+ 浅色舵轮(#E9EEF9,外环 R272-330 + 8 辐条半宽 19 至 R395 + 8 把手圆钮 R428±34 + 中心盘 R92)+ accent 蓝细节(#4C8DFF 中心盘 R48 与外环右上 28°-92° 高亮弧);4×4 超采样抗锯齿;输出 `assets/icon-1024.png`(49KB,留档可重生成)。
  2. **全套生成**:`npm run tauri icon assets/icon-1024.png` 覆盖 `src-tauri/icons/`(桌面 5 件:32/128/128@2x/icon.ico 含 6 尺寸/icon.icns + Windows Store Square*/StoreLogo + 移动端 android/ios 子目录,附赠无害);tauri.conf.json 引用的 5 个桌面文件全部就位。
  3. **验证**:ICO 结构合法(type=1,6 images);`touch build.rs && cargo build` 强制 tauri-build 重嵌资源,`ExtractAssociatedIcon` 读 exe 图标中心像素 `60,130,254` ≈ accent 蓝中心盘,确认新图标已嵌入(旧默认图标无蓝色)。
  4. **坑**:a) 角度弧度混用——`atan2` 已返回弧度,若再套 `deg()`(度→弧度)会得到 ~0.1 的恒小值,弧形条件永不成立且无报错,只能靠像素探针发现(采样环上 60° 处应为 accent 蓝);b) 改图标后须 `touch build.rs` 才能保证 tauri-build 重新嵌入 exe 资源;**release 打包(`cargo tauri build`)尚未重跑,下个打包节点自然带上**。
- [x] **P38 功能修复批二(P35 遗留项的高/中优先级 9 项,已完成,`cargo test` 61 项+`npm run build` 通过)**:
  1. **预览跨会话写错机器(高)**:`openFile` 把会话名存进 `preview.session`,`savePreview` 用它上传(不再用当前 activeTab);切换会话 `$effect` 清 `lastFile` 防「⬇ 下载/右键下载」作用于新会话同路径。
  2. **搜索方向(高)**:`doSearch` 不再无条件 `findNext` 再 `findPrevious`(此前首次按 ↑ 原地不动),prev 分支只调 `findPrevious`。
  3. **上传互斥(高)**:`uploading` 状态互斥(进行中再点提示等待)+ 按钮 disabled/文案「上传中…」+ `FileReader.onerror` 报错;上传循环用会话快照 `sess`(防上传中切标签写错机器)。
  4. **sftp_handle 并发双建通道(中)**:握手在锁外,写缓存前双检——另一调用已建好则返回现存、放弃本次重复通道(随 drop 关闭)。
  5. **read_file 短读(中)**:循环 `read` 读满 cap+1 或 EOF,不再单次 read(SFTP 分包短读致预览偏短+truncated 失真)。
  6. **create_dir_all 相对路径(中)**:不再把 `tmp/x` 拼成 `/tmp/x`(按输入是否 `/` 开头保留语义;前端恒绝对路径,属 API 层正确性修复)。
  7. **改名状态迁移(中)**:App `submitSession` 迁移 `pwds`(否则 Agent 初始目录退化为 "/")+ `qaStream.delete(old)`;`deleteSession` 同步清 `pwds/qaStream`。
  8. **Alt+I/L 输入框守卫(中)**:`handleKey` 两分支加 `&& !fromAppField`,重命名/搜索框内不再被劫持。
  9. **setActive 选已连接标签(中)**:新增 `pickNextActive(exclude)`(筛 `statuses[t]==="Connected"`),断开/关闭/删除标签后活动标签让给已连接会话,无候选 `clearActive()`(此前 `setActive("")`/指向未连接会话致输入路由错)。
- [x] **P39 AI 交互重设计:常驻命令条 + 活动流(已完成,纯前端零后端改动,`npm run build` 通过 + CDP 事件注入全链路验证)**。**方向**:消除 P25 内嵌方案遗留的三个违和根源——入口隐形(Alt+I 隐藏快捷键)、布局跳动(状态条忙显闲隐)、AI 输出与用户输出混流(Agent 命令/输出直接刷终端)。**新交互**(取代 P25 的条件渲染状态条 + Alt+I 弹出任务行 + 状态栏模式切换):
  1. **组件**:`AiCopilot.svelte` 全量重写为「AI dock」,固定渲染于 **term-area 之后**(终端区底部,流量布局——活动流展开挤压终端,termArea 已有 ResizeObserver 自动 refit):上=活动流 `.ai-stream`(max-height 38vh,头部任务描述/摘要/清空/收起,身体=卡片列表自动滚底),下=命令条 `.ai-bar`(34px 常驻:模式 pills「⌘ 问答/▶ Agent」+ 输入框(placeholder 随模式,busy 禁用)+ 发送/⏹停止切换 + 日志 + 活动流收展)。**Alt+I = 聚焦输入框**(`aiFocusSeq` 递增 → AiCopilot $effect focus),Alt+L 日志不变。
  2. **卡片模型**(api.ts 新类型 `AiCard = AiQaCard | AiStepCard`):QA 卡(kind qa,text 流式追加,done 收尾去光标);步骤卡(kind step,command + status `running/ok/fail/skipped/confirm` + message + output 折叠展开 + confirm 态带 level/reason 与「跳过/执行」按钮)。**危险确认从状态条移进卡片**。
  3. **App 状态流**:新 `aiCards/aiTaskText/aiSummary/aiStreamOpen/aiFocusSeq`;删 `aiPending/aiTaskOpen/qaStream`。onAi 映射:streaming(qa)→追加 QA 卡;commandStep→倒序匹配同命令 running/confirm/skipped 卡就地更新,无则新建;pendingCommand→push confirm 卡;done/error→`aiSummary` + QA 卡收尾;busy→输入框/按钮态。**终端保持纯净**:QA 回答与命令输出不再 `pushEcho` 进终端,Agent 任务仅在提交时回显一行灰色 `[AI] 任务: …` 锚点。
  4. **接线**:TerminalTabs props 换新(aiCards/aiTaskText/aiSummary/aiStreamOpen/aiFocusSeq/onDockSubmit/onToggleStream,删 onTaskSubmit/onCloseTask/onCancel);任务输入行模板与样式全删;StatusBar 移除 AI 模式胶囊(恢复纯状态显示,props 删 aiMode/aiBusy/onModeChange)。`submitFromDock`:重置卡片流→agent 留锚点→`api.aiSubmit(text, pwds[activeTab])`,失败置 `aiSummary` 错误;`decide` 乐观更新 confirm 卡(approve→running,reject→skipped,后端 CommandStep 统一收口)。
  5. **验证**(CDP 事件注入 `plugin:event|emit` ai 事件序列):空闲态 dock 常驻/状态栏无 AI;busy+pendingCommand→confirm 卡(⚠ 警告·理由 + 跳过/执行)+ 停止按钮 + 输入禁用;点执行→running⏳→commandStep×2→ok✓(输出按钮展开 pre.out)→done→摘要「✓ 任务完成」→busy false 输入恢复;QA streaming 双 chunk 合并一卡「磁盘使用率 42%,无需清理」;活动流收起/展开正常。截图确认视觉。
  6. **坑**:a) `commandStep` 的倒序匹配用 `[...aiCards].reverse().findIndex` 再换算回原下标(`length-1-idx`),直接在原数组 reverse 会改引用;b) confirm 卡点「执行」后端无事件回执,须**乐观置 running**,否则卡片停在 confirm 态直到 CommandStep;c) StatusBar 的 `--accent-dim` 变量保留(dock 未用,别处可用);d) 活动流是流式布局非 overlay,依赖 termArea 的 ResizeObserver(P17 fs-resize 同款)触发 xterm refit,该观察器已存在勿删。
- [x] **P40 全局 UI 重设计(剩余组件统一设计语言,已完成,纯前端,`npm run build` 通过 + CDP 断言/截图验证)**。与 P39 同一语言:SVG 线性图标替代 emoji、胶囊/幽灵/accent 三级按钮、卡片化列表、毛玻璃遮罩、统一 modal 结构(标题栏+×+body+footer)。
  1. **FileBrowser 重设计**:**上传/下载收进头部**(此前右下角浮动 absolute 按钮遮挡文件列表,已删),与 ↑⟳＋ 同排 icon-btn + 分隔线;路径从独立行改为头部内嵌**胶囊**(mono 输入框样式,truncat);折叠条 = 蓝色 SVG 文件夹 + 「文件」;列表加 **sticky 列头行**(名称/大小/权限/修改时间)、行内 SVG 图标(目录 accent `class:dir`、文件 muted)、行圆角+hover+选中 accent 左条(与会话项一致);空/加载提示移入列表容器内。**预览弹窗现代化**:720px、头部(文件图标+名称+mono 路径子行+×)、编辑区、**footer**(截断警告/已保存 + 下载 + 保存 primary)。
  2. **SessionForm/SettingsModal 统一 modal 结构**:标题栏(h3 + ×[设置:tabs pills 居右])+ body(可滚)+ footer(取消 ghost/保存 primary);遮罩改深色 + `backdrop-filter: blur(2px)`;输入聚焦 border 过渡;按钮统一(bg-panel 底/hover/accent primary/加粗)。
  3. **SysMonitor 打磨**:指标间细分隔线;`CPU/MEM/负载` 标签小字 muted 字距;数值 tabular-nums;网速 ↑ 绿 ↓ 蓝;**过热变色**(CPU>80%/MEM>85% bar 与轨道转 `--warning`)。
  4. **终端搜索改浮动组件**:search-bar 从 tabbar 下整条改 **term-area 内 absolute 右上浮层**(340px,modal-bg+圆角+阴影+放大镜 SVG,× hover 红),不再压缩终端高度。
  5. **AI 日志卡片化**:背景 bg-panel,条目改卡片(border + 左侧状态色条 + bubble 底),命令 code 保持,输出块换 term-bg mono,会话名 accent。
  6. **坑**:a) Svelte `{#const}` 只能直接用于 if/each/snippet 块内,**元素子级位置非法**(编译报 Expected 'if','each'...),内联表达式替代;b) 大段替换 style 块注意旧选择器残留(本次 SessionForm 替换后遗留孤立 `}` 与 ghost:hover 重复,构建 CSS 解析才暴露);c) SysMonitor 注入假数据须 name 匹配 activeTab(无标签时 activeTab=null 不显示,属预期);d) 搜索浮层 z-index 8 > ai-log 5,两者可叠。
- [x] **P41 项目文件整理(已完成)**:根目录从 30+ 杂项收敛到纯项目文件。
  1. **删除**:12 个历史 dev 日志(dev*.txt/dev.err/dev.log/deverr.txt/devlog.txt/helm-dev*.log/ui-dev*.{log,err})+ 重复截图 ui-after2.png(与 ui-after 字节相同)。
  2. **归档**:7 张 UI 验证截图 → `docs/screenshots/`(ui-before/after/connected/dock-pending/dock-final/redesign/settings)。
  3. **新增 `.gitignore`**(项目尚未 git init,为将来入库做准备):node_modules/dist/target/gen schemas/开发日志(*.log/*.err/dev*.txt)/系统杂项。
  4. **修正两处文档错误**:**known_hosts 实际持久化于 config.yaml 同目录的 `known_hosts.json`(JSON 格式,非 §5 P26-3 所写 `~/.config/helm/known_hosts.yaml`)**,known_hosts.rs 头注释为准;README 同步修正。**根目录的 known_hosts.json 是活跃 TOFU 数据(含 192.168.79.150 pin),勿删勿动**。
  5. **保留不动**:`dist/`(tauri.conf frontendDist 引用)、`assets/`(字体+图标源图)、`scripts/`(gen-icon/shot/reload-shot/eval 工具)、`.cargo/`、`.zcode/`、src-tauri 根(Cargo.lock/Cargo.toml/build.rs/tauri.conf.json,无测试残留副本)。
- [x] **P42 结构化重构:按域模块化,提升可拓展性(已完成,行为零变化,`cargo test` 61 项+`npm run build` 通过)**。动机:P41 只清了垃圾文件,巨石文件仍在——core.rs 1125 行(命令+AI 编排混杂)、ssh.rs 936 行(连接/PTY/SFTP/Agent 执行混杂)、api.ts 单文件、组件平铺。
  1. **Rust 拆分**(main.rs 注册 `mod ai_job; mod task_exec;`):
     - **`task_exec.rs`(新,自 ssh.rs)**:Agent 命令执行域——`TaskExecResult`/`task_exec_cmd`/`task_exec_cmd_windows`/`encode_powershell`/`trim_to_tail`/`parse_task_output`/`run_task_exec` + `MAX_TASK_OUTPUT_RETAIN` + 9 个纯函数测试整体迁移;ssh.rs(715 行)只留连接/PTY/exec/SFTP 通道管理。**新增平台分支只改 task_exec.rs**。
     - **`ai_job.rs`(新,自 core.rs)**:AI 任务编排域——`TaskCtx` + `run_ai_job`(QA/Agent 状态机)+ 局部 `task_exec` 辅助;从 core.rs import `AiControl/AiPayload`(模块交叉引用合法);core.rs(847 行)只留 CoreState/事件负载/薄命令/poller。**新增任务模式只改 ai_job.rs**。
     - 坑:拆出后 `run_ai_job` 须显式 `pub(crate)`;core.rs 原有的 `run_task_exec/parse_commands/truncate_text/check_danger/UnboundedReceiver` import 随之清理(unused warning)。
  2. **前端 lib 拆分**:`api.ts` → `types.ts`(全部类型)+ `commands.ts`(invoke)+ `events.ts`(listen)+ `api.ts` 桶导出(`export *`)——**所有现有 `../lib/api` 导入零改动**,新代码可按域直连。
  3. **组件分域**:`components/{terminal/{TerminalTabs,SysMonitor,AiCopilot}, sessions/{SessionPanel,SessionForm}, files/FileBrowser, chrome/{StatusBar,SettingsModal}}`;App.svelte 六个导入改域路径,组件内 `../lib/api`→`../../lib/api`(TerminalTabs→SysMonitor/AiCopilot 同域相对路径不变)。
  4. **验证**:`cargo build` 零警告 + `cargo test` 61 项全过 + `npm run build` 通过;行为零变化(纯移动,唯一代码改动是可见性与 import)。
- [x] **P43 Git 初始化 + 许可证(已完成)**:补齐项目缺的版本控制与法律文件。
  1. **git init + 初始提交**(5bca1d4):108 文件入库;**.gitignore 排除运行时本地数据 `config.yaml`(含会话明文密码样例)与 `known_hosts.json`(主机 pin)**,仓库内提供脱敏 `config.example.yaml`(password: null)代替;.gitattributes 统一 LF(`* text=auto eol=lf`,消除 Windows CRLF 噪音);`.zcode/` 不入库;LICENSE(MIT,与 Cargo.toml 声明一致)补齐。
  2. **后续开发流程**:改完一批 → git add -A + commit(AGENTS.md 同步更新一起提交);发布时打 tag。**AGENTS.md 一并入库,新机器 clone 后按档案即可恢复全部上下文**。
  3. **远程仓库**:`https://github.com/901548/Helm`(origin/main,本地分支 main);首推时远端已有建仓占位提交(303be40,stub README/LICENSE),经 `git merge --allow-unrelated-histories -X ours` 合并(README/LICENSE 保留本地完整版);LICENSE 版权人对齐 901548。**config.yaml/known_hosts.json 已 gitignore 不在远端,clone 后需自备 config.yaml(结构见 config.example.yaml)**。
  3. **坑**:a) git 在 Windows 无 .gitattributes 时按 core.autocrlf 全量警告 CRLF 转换,入库前先建 .gitattributes 最省心;b) `git rm --cached` 只退出暂存不删本地文件(config.yaml/known_hosts.json 仍在磁盘上供应用运行);c) 内联 node 脚本里写反引号会被 bash 当命令替换吃掉,长文本操作一律走 Edit/Write 工具(P20 教训重演)。
- [x] **P44 前端测试 + CI + release(已完成,`cargo tauri build` 三件套 + tag v0.1.0 已推送;CI 首跑状态需在仓库 Actions 页确认——仓库为私有,匿名 API 不可见)**:
  1. **前端纯函数抽离 + Vitest**:TerminalTabs/FileBrowser 内嵌纯函数抽到 `lib/osc.ts`(stripAnsi/parseOscPwd/normalizePwd——P24 跨 TCP 分包解析,前端最易错逻辑)与 `lib/paths.ts`(joinPath/resolvePath/parentPathWindows/shq/fmtSize);组件改薄包装(import + 闭包注入 sep/cwd);`osc.test.ts` + `paths.test.ts` 共 **26 项**(跨包 OSC 拼接/多标记取尾/非 helm 前缀不误取/盘符路径/shq 转义等,P24 真实场景全覆盖);`npm test` = `vitest run`。**测试抓到原实现怪癖**:`~/data/` 因早返回不剥尾斜杠,lib 版修正(~ 路径也剥)。
  2. **CI**:`.github/workflows/ci.yml`(windows-latest + Node 22 + rust stable):npm ci → npm test → npm run build(**dist 必须先于 cargo test 存在,generate_context! 编译期嵌入前端产物**)→ cargo test。**.cargo/config.toml 仅配置 gnu target 链接器,对 CI 默认 msvc 宿主构建不生效,无需改**。
  3. **release 构建**:`cargo tauri build` 带新图标出三件套(P37 遗留补跑)——`helm.exe`(13.5MB,图标中心像素 60,130,254 = accent 蓝确认新图标)+ `Helm_0.1.0_x64_en-US.msi`(5.8MB)+ `Helm_0.1.0_x64-setup.exe`(4.2MB);tag v0.1.0。
  4. **坑**:a) 本机 gh CLI 未安装,GitHub Release 网页创建或后续装 gh;b) GitHub 443 间歇不可达(无代理环境),推送失败安全——本地提交不丢,网络恢复 `git push` 即可,勿因此重写历史。
- [x] **P45 服务器恢复真机冒烟:抓出并修复 P29 安全回归(已完成,`cargo test` 61 项含 5 个 live 全过 + CDP 真机全链路验证)**:
  1. **P29 安全回归(高危,文件删除被全面拦截)**:live 测试 `sftp_live_readwrite` 失败暴露——P29 把「任何 `/` 开头目标」判为 Critical 根删除,而 `fs.rs remove()` 拦截 Critical → **文件面板删除任何绝对路径目录必被拦截**(UI 恒用绝对路径,自 P29 起实际不可用)。P29-P44 期间未暴露纯属服务器离线 live 跳过。**修复**:`has_root_target` 收窄为真清根(`/` 本身或 `/*`/`/**` 通配),普通绝对路径回落 Warning;`critical_rm_variants`/`warning_rm_variants`/`chmod_commands` 断言同步(`/tmp/a`→Warning,`chmod 777 /etc`→Warning,新增 `/*` 与 `chmod 777 /` Critical)。
  2. **remove 幂等**:`remove_recursive` 对 symlink_metadata 的 "No such file" 视为成功(重复删除/测试起始清理不再报错);测试起始清理从 `let _ =` 静默改为显式断言报错(本次盲区即源于静默吞错)。
  3. **真机全链路(CDP @9229,192.168.79.150)**:连接(慢认证 ~20s, dot ok + tab + 提示符);`hostname -I` 回显;**双向 cd 联动**(终端 `cd /etc`→面板 /etc 177 行;双击 alternatives→面板+终端 `cd '/etc/alternatives'`——P44 osc.ts 抽离后依然正确);**mkdir** 创建 `helm-p45`(P38 修复验证:无 `undefined` 目录);**右键删除** helm-p45 成功(P45 修复验证:不再被 Critical 拦截);SysMonitor 实时(CPU 0%/MEM 414M/3.7G/负载/网速);**AI dock E2E**(QA 提交→活动流 QA 卡+「✗ AI 未配置」摘要,错误路径即验证);截图 `docs/screenshots/ui-live-connected.png`。**仍待办:Windows 测试机冒烟(P34 项)+ 真实 AI 多步任务(需 API Key)**。
  4. **坑**:a) 测试起始清理 `let _ = remove(...)` 吞错是排查最大障碍——清理性前置操作必须显式断言;b) 语义收窄后 `rm -rf /tmp/a/../..` 这类**路径回溯写法分词层判不出**(resolve 后才是 /),已知取舍,Critical 仍覆盖直接形式;c) cargo test live 前须 `cp config.yaml src-tauri/`(P28 坑重申)。
- [x] **P46 遗留问题批 + v0.1.1 重打包(已完成,`cargo test` 61 项含 live 全过;CI 经 git credential 查得三次运行全绿;v0.1.0 安装包含 P29 回归 → v0.1.1 替换)**:
  1. **小修 6 项**:a) **连接双击守卫**:SshManager 加 `connecting: HashSet`,`mark_connecting` 返回 bool(false=已有任务,connect_session 早退),connect 成败出口 `clear_connecting`;b) **shell 尾输出**:读任务 ExitStatus 不再 break,等 EOF(None) 才退出(部分 shell 在退出状态后 flush 尾部);c) **下载上限**:download 先 metadata 检查 >64MB 报错(此前整文件入内存无上限);d) **update_session 旧名不存在显式 Err**(此前静默成功);e) **监控 prev 泄漏**:断开即 `prev.remove`;f) **reqwest Client 复用**:Agent 持有 `client` 字段构造期建立(build_client 改关联函数),每步不再重建 TLS。
  2. **审查纠错**:「窗口宽高设置不生效」为误报——`update_ui_config` 本就 `set_size`,划掉不修。
  3. **v0.1.1**:三处 version bump(package.json/tauri.conf.json/Cargo.toml)→ `cargo tauri build` 出 `Helm_0.1.1_x64_en-US.msi`(5.8MB)/`Helm_0.1.1_x64-setup.exe`(4.2MB),**含 P45 删除修复,取代带 bug 的 v0.1.0**;tag v0.1.1 已推送。
  4. **CI 验证**:`git credential fill` 取本机 token 调 GitHub API——**私有仓库也能查 Actions**:三次运行(run 1/2/3)全部 success(含 P45 提交),msvc 构建 + Vitest + cargo test 在 windows runner 上无需任何适配。
  5. **坑**:a) JS `String.replace(字符串,...)` 只替换**第一处**,批量改代码用 split/join(P46 就漏了第二个 `self.build_client()` 调用点);b) russh-sftp File 无 `read_all`,读整文件用 `sftp.read(path)`;c) GitHub 443 间歇阻断时 tag 与 main 分开重试(本次 tag 先通、main 后通),本地提交安全勿重写历史。**遗留未修(有意)**:Agent 危险检测不覆盖 `$()`/xargs(需语义级方案)、ai_busy TOCTOU(UI 已挡)、QA 持 agent 锁、known_hosts pending_error 单槽。
- [x] **P47 清零批(P46 遗留 4 项全部修复,`cargo test` 63 项全过;live 因服务器再度离线自动跳过——改动均为纯函数/锁语义级,单测覆盖)**:
  1. **safety.rs 二次执行防护**:a) **xargs 管道合并分析**——段含 `xargs` + 危险命令(rm/chmod/dd/mkfs/shutdown/reboot)时,把整条命令的 `|`/`;`/换行替换为空格后合并分词判定(`echo / | xargs rm -rf`→Critical;`find /var/log -name '*.gz' | xargs rm -rf`→Warning 不误伤日常清理;`echo x | xargs cat`→Safe);b) **命令替换根目标检测**——危险命令的 `$(...)`/反引号体内出现根 token(`/`、`/*`)即升级 Critical(`rm -rf $(echo /)`);`rm -rf $(pwd)/build`/`$(echo /tmp/a)` 维持 Warning;`echo $(ls /)` 不升级。**check_segment 拆出 check_tokens(tokens) 供合并分析复用**。+2 单测(xargs_secondary_execution / command_substitution_root)。
  2. **ai_busy TOCTOU**:`ai_submit` 改 `compare_exchange(false,true)` 原子抢占,并发提交只一个进入;早退路径(空输入/无会话)统一 `rollback` 复位 busy + 发 Busy{false}(否则任务永久"忙")。
  3. **QA 持锁不再阻塞查询**:CoreState 增 `ai_mode_agent: AtomicBool` 缓存(构造期从 config 初始化,ai_set_mode 双写);`ai_mode` 免锁即时返回(此前 QA 聊天持 agent 锁最长 60s 会卡住查询);`update_ai_config` 加 busy 守卫(任务中改配置与进行中的请求互踩)。
  4. **known_hosts 错误通道**:`verify/verify_fingerprint` 改返回 `Result<(), String>`,失败原因经返回值直达本次连接的 key_error 槽;**删除共享 `pending_error` 单槽与 `take_pending_error`**(设计上天然并发安全,不再依赖锁内 set+take 的时序巧合)。
  5. **坑**:a) `trim_matches` 会剥**所有**满足谓词的首尾字符——用「非/非*」做谓词时 `$(pwd)/build` 被剥成 `/` 误判根;包装标点剥离须排除字母数字(`!c.is_ascii_alphanumeric() && c != '/' && c != '*'`);b) 测试调试用临时 dbg_tests + `-- --nocapture` 打印分类路径,定位后删除,勿留库中。
- [x] **P48 多提供商 UX + 测试连接(已完成,`cargo test` 65 项全过 + `npm run build`/Vitest 26 项通过;CDP 真机全链路验证)**:
  1. **动机**:用户反馈「不能只接 deepseek」+ 想要配置后立即验证可用性。核实:agent.rs 本就是通用 OpenAI 兼容客户端(默认 base 甚至不是 DeepSeek),缺的是**可感知的提供商入口**与**连通性测试**。
  2. **agent.rs**:a) **`resolve_api_key` 提取为关联函数**——api_key 字段(DPAPI 密文→解密,**明文→原样**,手改 config.yaml 的明文 key 从此也能用)> 环境变量 > **本地服务免 Key**(base_url host 为 localhost/127.0.0.1/0.0.0.0/::1 时返回空串,`is_local_base` 判定,Ollama/LM Studio 零配置可用);b) `Agent::new` 改用之,免 Key 不再阻塞本地服务;c) **call_api_plain/call_api_stream 空 key 时不发 Authorization 头**;d) 新增 `pub async fn test_connection(&AiConfig) -> Result<String,String>`——最小探测(一条 "hi" + max_tokens=1 非流式),超时 `timeout_secs.max(20)`(下限 20s:**本地 LLM 冷启动实测 >15s**,固定 15s 会误报失败),HTTP 非 2xx 返回截断 300 字符错误,2xx 读响应 `model` 回显拼「连接成功（{model}）」。+2 单测(local_base_detection / resolve_key_prefers_field_over_env)。
  3. **core.rs**:新 command `test_ai_connection(model, api_base_url, api_key: Option<String>)`——已保存配置为底、表单非空值覆盖(api_key 空/哨兵 `·` = 沿用已存密文);**只读探测:不落盘、不重建 Agent、不受 ai_busy 限制**。main.rs 注册。
  4. **前端**:commands.ts `testAiConnection`;SettingsModal AI tab 顶部加**提供商快速填入下拉**(10 预设:DeepSeek/OpenAI/Google Gemini/Groq/Ollama本地/硅基流动/智谱GLM/通义千问/月之暗面Kimi/零一万物Yi,选中自动填 model+base URL 后 select 复位便于重选);model/URL placeholder 改通用文案;**「测试连接」按钮**(API Base URL 下方,用表单当前值即未保存也可测,测试中禁用,结果绿 ✓/红 ✗ 行内展示,改三个相关输入自动清除旧结果)。
  5. **验证(CDP @9229,本机 Ollama 恰在 11434 跑 qwen3:8b)**:预设填入 ✓;绿色成功 `✓ 连接成功（qwen3:8b）`(含 model 回显)✓;红色 404 `model not found` 截断展示 ✓;远端无 Key `✗ 未配置 API Key（请填写 API Key 或设置环境变量 DEEPSEEK_API_KEY）`(合并回退逻辑正确)✓;本地免 Key 直达 API(无 Key 报错)✓;超时路径 ✓。
  6. **坑**:a) **CDP 启动须带 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9229"` 环境变量**,否则 helm.exe 起来但 9229 永远不通(本次忘了带白等 150s);b) Ollama **串行处理**排队的请求——前一次 15s 超时放弃的请求服务器还在跑,紧跟的第二次探测排队又超时,复测前先 curl 计时确认队列清空(实测直连 11.5s);c) Svelte select 预设选中后 `sel.value=""` 复位,否则重复选同一项不触发 change;d) CDP eval 里 NodeList 直接 `[1]` 索引偶发 undefined,先 `[...nodeList]` 展开再取。
- [x] **P49 模型列表拉取(GET /models,已完成,`cargo test` 66 项全过 + `npm run build` 通过;CDP 冒烟拉到本机 Ollama 真实模型)**:
  1. **动机**:P48 解决了「选哪家提供商」,但模型名仍要手敲猜——本地 Ollama 装了哪些 tag、云厂商模型名怎么拼,用户无从知道。OpenAI 兼容生态有标准 `GET {base}/models` 端点,拉真实列表选择即可。
  2. **agent.rs**:a) **`parse_models_response(text)`** 纯函数——解析 OpenAI 格式 `{"data":[{"id":"..."}]}`,取非空 id 排序返回;缺 data 数组报「该服务可能不支持 /models 端点」、非 JSON 报错(不 panic);b) **`Agent::list_models(&AiConfig) -> Result<Vec<String>,String>`**——GET `{base}/models`(base 剥尾斜杠拼接),key 解析复用 `resolve_api_key`(**空 key 不发 Authorization 头**,本地服务免 Key),超时 `timeout_secs.max(20)`(与 test_connection 同规则),HTTP 非 2xx 截断 300 字符,空列表报「服务未返回任何模型」。+1 单测(parse_models_response_sorts_and_filters)。
  3. **core.rs**:**`merged_ai_config` 提取为公共辅助**(test_ai_connection 原合并逻辑抽函数复用——已保存配置为底、表单非空值覆盖、api_key 空/哨兵 `·` 沿用已存密文);新 command **`ai_list_models(model, api_base_url, api_key)`** 与 test_ai_connection 完全同规则(**只读探测:不落盘、不重建 Agent、不受 ai_busy 限制**)。main.rs 注册。
  4. **前端**:commands.ts `aiListModels`(参数规则同 testAiConnection);SettingsModal 模型输入框旁加**「获取列表」按钮**(loading 态「获取中…」disabled),拉到后下方显示**选择下拉**(「从列表选择模型…（N 个）」+ 全部模型 option,选中即填入 model 输入框且 select 复位空值便于重选);失败红 ✗ 行内展示;**改 provider 预设时清除已拉取的列表与错误**(模型列表属于旧 base,留着误导)。用表单当前值即未保存也可拉。
  5. **验证(CDP 冒烟,本机 Ollama 11434)**:点「获取列表」拉到 5 个真实模型(deepseek-r1:8b / gemma4:26b / qwen3:8b 等)下拉展示;选中即回填输入框;`cargo test` 66 项(65+1)全过 + `npm run build` 通过。
  6. **坑**:a) 模型选择 select 选中后须复位 `e.currentTarget.value=""`,否则重复选同一模型不触发 onchange(与 P48 预设下拉同坑);b) `list_models` 用 `Self::build_client(&probe_cfg)` 独立建 client(非 Agent 实例方法语义,关联函数静态调用,与 test_connection 一致);c) Ollama 的 `/models` 响应天然 OpenAI 格式(`data[].id`),本地与云厂商端点同构,无需分支。
- [x] **P50 提供商预设下拉"选不上"修复(已完成,CDP 全断言 PASS + `npm run build` 通过;纯前端单文件改动)**:
  1. **现象**:用户反馈提供商下拉"只能下拉没办法选择"。根因:P48 的 `applyPreset` 在 change 里读值后**立即 `sel.value=""` 复位**——点选任何提供商,下拉瞬间弹回占位符"选择提供商…",感知上就是选不上(功能其实已填入 model/URL,但控件从不显示所选)。
  2. **修复(SettingsModal.svelte)**:预设下拉改 **`bind:value={presetSel}` 持久选中**(与主题/初始模式下拉同模式),删除中途复位;`onPresetChange` 从 `e.currentTarget.value` 读值填 model/apiBaseUrl 并清 testResult/modelOptions/modelsError;打开弹窗时按已存 `api_base_url`(trim 相等)**预选匹配预设**;手改 base URL(oninput)**读 `e.currentTarget.value`** 与所选预设 base 不一致时 presetSel 回占位符(程序化赋值不触发 oninput,无循环)。
  3. **验证(CDP:vite dev + debug exe @9229)**:选 DeepSeek→下拉显示 DeepSeek + 填入 deepseek-chat/api.deepseek.com;切 OpenAI 正常;手改 base→回占位符;重选 Gemini 正常;弹窗关闭无残留;console 零错误。
  4. **坑**:a) **Svelte 5 `bind:value` 对 option 值严格匹配**:`{#each ... as p, i}<option value={i}>`(数字)与字符串状态 `"0"` 不等 → select 被置为无匹配值,**selectedIndex=-1 显示空白**(不是回落占位符!修复:option value 一律 `String(i)`);b) **debug exe 走 devUrl**:调试构建按 tauri.conf 连 localhost:1420,vite 没跑时 WebView 显示 `chrome-error://chromewebdata`(标题"localhost"、只有一个"刷新"按钮),CDP 测的是错误页还查不出异常——**CDP 前先 `npm run dev` 或改用 release exe(embedded dist)**;c) 自定义 oninput 与 bind:value 的 input 监听执行顺序不保证,handler 里读状态变量会踩旧值,一律读 `e.currentTarget.value`。
- [x] **P51 本地 Ollama 联调批(已完成,`cargo test` 70 项+`npm run build` 通过;commit fe02fab)**:推理型模型思考过程实时显示(`AiStreamEvent::Reasoning`,`AiPayload::Reasoning`);Agent 命令回显防死循环(`is_command_line` 过滤提示词回显/散文行 + `contains_done_line` 按行识 DONE + 连续无有效命令即中止);Agent 命令镜像进终端(`[AI] $` 青/绿红标注);AI 活动流高度封顶 `min(38vh,300px)` 收拢布局。
- [x] **P52 Agent 专属提示词(已完成,commit 17088a0)**:agent.rs 加内置 `default_system_prompt_agent()`(config 未配 `system_prompt_agent` 时不再复用 QA 提示词);本地 `config.yaml` 亦写入专属提示词。
- [x] **P53 会话折叠柄 UI 打磨(已完成,commit 0a0cdf9)**:折叠栏收回宽度归 0,左上角 28×28 圆角矩形展开柄。
- [x] **P54 Docker 容器连接(首版,已完成,`cargo test` 71 项+`npm run build` 通过;未提交)**:`SessionKind::Docker` + `SessionInfo.container`(config.rs,serde default 兼容旧配置);`task_exec.rs` 新增 `docker_exec_cmd`(base64 编码后经容器 `sh -c` 解码执行,避开引号冲突);core.rs Docker 会话连接跳过 PTY 终端(首版无容器交互终端),`TaskCtx.container` 透传;ai_job 经 `docker exec -i <容器> sh` 注入执行。
- [x] **P55 多会话并行 + 事件按会话隔离(已完成,commit a2e0c3b + 后续未提交修复)**:core.rs 全局 AI 状态改 `AiManager`(每会话懒建 `AiSlot`,各自独立 `Agent/busy/mode/ctl`);所有 AI 命令(`ai_submit/ai_control/ai_stop/ai_clear_history/ai_set_mode/ai_mode`)带会话名;`update_ai_config` 只重建空闲槽。修复:前端 `aiBusy` 由单一布尔改 `Record<会话名,boolean>`(按会话存,提交只拦当前会话);`AiPayload` 非 busy 事件统一带 `name`,前端面板按 `activeTab` 过滤、镜像写回各自会话终端;`delete_session/rename_session` 补 `ai.remove` 清理 AI 槽。
- [x] **P56 §8 AI 状态机落地(已完成,`cargo test` 71 项+`npm run build` 通过;未提交)**:core.rs 加 `AiRunState` 枚举 + `AiPayload::{State,Planning}` + `AiControl::Edit(Vec<String>)`,`ai_control` 加 `"edit"` 分支,`ai_submit` 加 `container` 覆盖;ai_job `run_ai_job` 用 `set_state()` 显式迁态(§8.7.1 映射),解析命令后广播 `Planning` 计划卡(§8.7.2),计划级一次性确认 `Approve/Edit/Reject/Cancel`(§8.7.3,裁决为"**一次批准 + 编辑回退 + 严格模式逐条**":批准整份=对本步全部命令(含危险标记)一次性显式授权,后续不再二次逐条确认,消除"双确认"冲突;`Edit` 复位 `preapproved`,编辑后新增危险命令重新逐条兜底,绝不因编辑绕过授权;`confirm_all` 严格模式恒逐条确认);前端 AiCopilot 渲染计划卡(修改/查看命令/放弃/执行)+ Dock 容器输入(Docker 会话运行时选容器,§8.7.4)。**同会话多执行体暂缓**(§8.2 并发语义本期按"每会话单执行体 + 跨会话 name 隔离"落地)。
- [x] **P57 L2-alpha 长任务能力子目标化 + 失败恢复(已完成,`cargo build`+`cargo test` 86 项+`npm run build` 通过;未提交,真机已验证)**:agent.rs 增 `extract_goals`/`extract_reflexion`(解析模型回复的 `GOAL <序号> <短标题>` 子目标行与 `REFLEXION <原因>对策>` 反思行,`GOAL_` 前缀命令过滤防误执行);ai_job `run_ai_job` 建子目标任务图(内存级 `GoalStatus{Pending/Active/Ok/Failed}`,`next_active_goal` 取首个未完成子目标,当前子目标结束按执行成败标记 `Ok/Failed`,`failed` 时结合 `last_reflexion` 重规划当前子目标,不做无意义重复重试;新增 `apply_goal_outcome` + 每子目标 `go_fail_retries`,**失败超 `MAX_GOAL_FAIL_RETRIES=2` 即放弃该子目标置 Ok 跳过**,防模型漏发完成信号/反复重试同一失败命令烧穿 max_steps,保证收敛)、确定性校验(命令按**真实退出码**判定成功而非模型自述,根除"模型自夸完成")、上下文压缩控制长任务占用;config.yaml 改写 `system_prompt_agent` 引导子目标分解+GOAL_OK+REFLEXION 输出协议。core.rs 增 `AiPayload::{GoalStarted,GoalDone}` 事件、agent.rs 补 `strip_goal_number`(char 索引用法避字节越界 panic)+ 大小写不敏感解析;前端 App.svelte/types.ts 加 `goalStarted`/`goalDone` 事件处理,子目标开始/结束以青/绿色标注镜像进各自会话终端。**真机验证(192.168.79.150,修复后复测)**:长任务"建 a/b/c 目录→写 hello.txt→列目录树→统计字符数"被拆为 4 子目标顺序推进;`tree -L 3` 返回**退出码 127 失败**后 Agent 自动 `REFLEXION` 复盘改用 `ls -R` 完成目录树列举(**失败恢复真机生效**);`wc -c .../hello.txt` 输出 11(10 字符+换行,正确);子目标序号显示正常。**修复子目标序号显示 NaN bug**:`AiPayload` 枚举级 `rename_all="camelCase"` 只重命名变体 tag,不含结构体字段 → `goal_index` 序列化后前端读 `p.goalIndex` 得 `undefined`→终端显示 `子目标 NaN`;给 `GoalStarted/GoalDone` 变体加 `#[serde(rename_all="camelCase")]` 使字段名对齐后复测,子目标显示"3/4"等正确数字,任务整体 `✓ 任务完成`。
- [x] **P59 二轮审计加固:幽灵连接/路径删除绕过/config 并发写/凭据脱敏/执行安全(已完成,`cargo test` 87 项+`npm run build` 通过,未提交)**:① **幽灵连接**:`ssh.connect` 成功写回前,先查 `connecting` 集再锁 `sessions`(锁序与 mark_connecting 一致防死锁),若占位会话在连接建立期间被删除/改名/断开(`sessions` 无该会话 但 `connecting` 有标记)则丢弃刚建立的句柄(Drop 关闭 SSH 传输)返回"连接已取消",不再重插已连接的幽灵会话(此前 45s 握手后重插→SSH 常驻泄漏+rename 残留旧名+点断开仍复活);测试直连(无占位)仍正常新建。② **路径删除绕过**:fs.rs `assert_removable` 由"只拦精确 `/`"改 `resolves_to_root`(POSIX 分量归一化:``|"."` 忽略、`..`上行触顶钳 0、普通分量下行,最终深度 0 且非空即根),拦截 `/`、`//`、`/./`、`/../`、`/..`、`a/b/../../..` 等全部根目录变体(此前 SFTP 递归可删到根);软链接仍用 symlink_metadata 不跟随。③ **config 原子写**:`save_config` 临时文件由固定 `yaml.tmp` 改 `yaml.tmp-{pid}-{序号}`(AtomicU64),并发保存(persist 锁外/update 锁内)不再双写同一 tmp 相互踩踏。④ **凭据脱敏**:`get_ai_config` 在屏蔽主 api_key 前解密出明文,把 `extra_headers` 中等于明文 key 的值同样置哨兵 `·`,防止用户把 key 放进 x-api-key/authorization 置明文下发前端)。⑤ **执行安全**:`run_task_exec`/`run_exec` 超时错误不再内联完整命令文本(防含 `-u user:pass` 的命令回显泄漏,经 ai_job message 回传前端/模型);`docker_exec_cmd` 容器名单引号转义包裹,防容器名含 shell 元字符(`$(…)/;/空格`)时注入。
- [x] **P60 三审加固(已完成,commit ff4923f)**:`read_file`/`read_snippet` 对不可信 `limit` 参数按 `preview_cap`(1MiB~16MiB+容量上限)钳制,杜绝超大 limit 触发内存 OOM abort;补 `preview_cap` 单元测试。
- [x] **P61 纯安全计划只读自动执行(已完成,commit d07cb08)**:ai_job 解析命令后 `need_confirm = confirm_all || 存在非 Safe 命令` 并写进 `AiPayload::Planning`;纯安全计划前端只读展示「安全·自动执行」蓝胶囊,隐藏 执行/放弃/修改 按钮(仅留「查看命令」),消除装饰性按钮假交互。
- [x] **P62 计划卡 needConfirm 序列化补 camelCase(已完成,commit 4276a4c)**:P61 为 `Planning` 新加 `need_confirm` 字段,但漏加 `#[serde(rename_all="camelCase")]`(与 P57 `GoalStarted/GoalDone` 同款陷阱),后端以 snake_case 序列化为 `need_confirm`→前端 `p.needConfirm` 得 `undefined`→`p.needConfirm ?? true` 兜底为 true→纯安全计划仍误显示执行/放弃/修改按钮。给 `Planning` 补 camelCase,新增 2 条回归断言(`needConfirm` 键存在、`need_confirm` snake 键不存在),`cargo test` 93 项全过;计算机真机端到端验证:agent 模式提交纯安全任务(uptime/cat hostname/ls -la /tmp)→3 张计划卡均显示「安全·自动执行」且无执行/放弃/修改按钮,截图 `docs` P0#1 关闭。
- [x] **P63 §8.7.1 状态机前端半边补齐(state 事件消费 + dock 状态 chip,已完成,CDP 五态注入 ALL PASS + Vitest 26 项/`npm run build` 通过)**:
  1. **缺口(UI 巡检发现)**:P56 只做了状态机后端半边——ai_job 在全部 6 个迁移点(行 226/411/422/463/577/593)广播 `AiPayload::State`,但 App.svelte 事件 switch **无 `case "state"`**,无人消费→前端仍靠 busy/cards 猜态,§8.2"唯一状态机、面板不判态"未成立,界面也无任务阶段指示。
  2. **修复(纯前端三跳)**:App.svelte 增 `aiState: Record<会话,AiState>`(与 aiBusy 同款按会话生命周期)+ `case "state"` 落盘 + 传 `aiState[activeTab]`(缺省 "idle");TerminalTabs 透传;AiCopilot 墽 `aiState` prop + 命令条**状态 chip**(模式胶囊与容器输入之间):`busy && state!=="idle"` 才显示,五态文案 解析中/生成计划/等待确认/执行中/读取回显,脉动圆点 accent 色,`awaitingConfirm` 转 `--warning` 警告色,idle 自动消失。
  3. **验证(CDP 事件注入)**:初始 idle 无 chip→busy+五态依次迁移,chip 文案逐一正确且 awaitingConfirm 带 wait 类→state idle+busy false 复位后 chip 消失,ALL PASS;截图 `docs/screenshots/ui-review-state-chip.png`。类型层本就对齐(`AiRunState` serde camelCase = TS `AiState`),零后端改动。
  4. **坑**:a) chip 只在 busy 时显示——后端 `Idle` 迁移先于 `Busy{false}` 发出,若只判 state 会闪一帧"有 state 无 busy";绑定 `busy &&` 双条件最稳;b) Edit 工具大段替换会吃行尾换行(本次 levelText 对象尾行被并排),替换后必须核对邻行结构。
- [x] **P64 逻辑审查双修复(已完成,`cargo build` 零警告 + `cargo test` 93 项全过)**:
  1. **任务收尾清理竞态(core.rs ai_submit,低概率高危)**:spawned 任务收尾在 `busy=false`(有 await 点位:ctl/task 锁)之后执行 `ctl=None`/`task=None`——若此刻执行器暂停旧任务、用户立刻提交新任务 B(CAS 成功写 B 的通道+句柄),旧任务恢复后把 **B 的控制通道与 JoinHandle 一并抹掉**→B 的危险命令确认/停止全部报"任务未在运行"(卡到 120s 超时)、删除会话 abort 不掉(僵尸任务)。**修复:删除这两行清理**——陈旧通道留着只让 ai_control 报"任务已结束"(语义正确),下次 submit 自然覆盖;陈旧 JoinHandle abort 已完成任务是 no-op。`busy=false`+`Busy{false}` 保留。
  2. **flush_stale 吞 Cancel(ai_job.rs)**:计划级确认前 drain 陈旧信号的闭包不检查 Cancel——用户在模型推理刚结束的窗口点「停止」,Cancel 被当陈旧信号吞掉,任务继续弹计划卡干等 120s(与逐条确认处 drain 检查 Cancel 的行为不一致)。**修复:flush_stale 返回 bool(是否见 Cancel),调用处 true 即 finished+break**。
  3. **审查确认不改**:严格模式(confirm_all)计划卡+逐条双确认是 P56 设计意图;container 参数有 kind 闸门(Linux 误传走 task_exec_cmd,无危害);授权链(批准=整份授权/Edit 复位 preapproved)闭合;退出码确定性判成败+重试上限+premature_done 上限收敛有界;账本/输出有截断上限;确认双超时保证 busy 必回落;sender 存活全任务期,recv() None 路径不可达。
  4. **遗留小瑕疵(评估过,暂不处理)**:premature_done 提示文案插值 goals.len() 而非剩余数(模型侧轻微失真);Linux 会话 dock 也渲染「容器(可选)」输入(输入无效果,纯观感)。
- [x] **P65 设置弹窗删窗口宽高输入(用户定,已完成,CDP 验证 PASS + `cargo test` 93 项/`npm run build` 通过)**:
  1. **理由**:P26-6 起窗口几何(宽高/位置)由"拖拽调整 + 退出自动落盘 + 启动恢复"管理,设置弹窗的手填宽高是冗余遗留;且保存时后端 `set_size` 会把窗口拉回表单陈旧值(挂载时加载的旧尺寸),拖大窗口后随手保存设置就被改回,属负收益。
  2. **改动**:SettingsModal UI tab 删「窗口」卡(仅含宽高两项);submit() 宽高透传 uiConfig(仅满足 TS 类型);**core.rs `update_ui_config` 去掉 `app: AppHandle` 参数与 set_size/set_position 调用,宽高与 xy 一律保留磁盘现值**(几何唯一写者 = main.rs Moved 事件/RunEvent::Exit 落盘),随删 core.rs 顶部不再使用的 `Manager` import。
  3. **验证(CDP)**:界面 tab 仅剩「布局」(会话面板宽度%/显示会话面板)与「外观」(主题)两卡,无几何输入;保存后视口尺寸不变;弹窗正常关闭。build 输出中 `state_referenced_locally` 两条为 P27 已知保留警告,与本次无关。
  4. **坑**:SettingsModal 模板在 P60-P62 已重构为卡片分组(sec-title),凭旧行号/旧结构写 Edit 会失配——改前端模板前先重读现文件。
- [x] **P66 设置页重设计:三 tab 信息架构(用户定,已完成,CDP 三 tab 断言 + 截图 + Vitest 26 项/`npm run build` 通过)**:
  1. **动机**:AI tab 五卡过长需滚动,连接三件套(提供商/模型/Key/测试)与专家调参争夺注意力;P65 删宽高后 UI tab 仅剩三控件,二 tab 结构失衡。
  2. **新结构(tab 默认 "conn")**:**「连接」**= 模型卡 + 认证卡(预设填入/获取列表/API Key/URL/测试连接,首屏零滚动,90% 用户到此为止);**「AI」**= 行为卡(初始模式/流式/全部确认/自定义提示词,概念项提前)+ 生成参数 + 执行限制(数值调参殿后);**「界面」**= 布局 + 外观(不变)。保存按钮仍是全量提交(跨 tab 生效,与 tab 无关)。
  3. **验证(CDP)**:默认开「连接」;三 tab 切换 sec-title 断言正确(连接=[模型,认证]/AI=[行为,生成参数,执行限制]/界面=[布局,外观]);界面 tab 点保存→全量落盘+关窗;截图 `docs/screenshots/ui-review-tab-{conn,ai,ui}.png`。预设预选联动(P50)在新结构下正常(用户配置 localhost:11434 → Ollama 预设选中显示)。
  4. **坑**:无——纯模板重组 + tab 状态类型扩展(`"conn"|"ai"|"ui"`),状态/提交逻辑零改动。
- [x] **P67 设置三项新增:终端设置/Agent 提示词/忘记主机密钥(已完成,`cargo test` 94 项+Vitest 26 项+CDP 全链路验证)**:
  1. **终端设置(界面 tab 新「终端」卡)**:config.rs `UiConfig` 加 `term_font_size`(default 14)/`term_scrollback`(default 5000,serde default 旧配置兼容,+1 单测 `ui_config_defaults_for_missing_term_fields`);SettingsModal 字体大小(钳 8-28)/滚动缓冲(钳 500-100000)两输入;TerminalTabs 收 props 替代硬编码(原 fontSize:14/scrollback:5000),**即时生效**仿主题 `$effect`(options.fontSize/scrollback 赋值 + tick 后 fitActive refit,无需重建终端);App 传 `uiConfig` 字段(saveSettings 整体替换自动触发)。
  2. **Agent 提示词编辑(AI tab 行为卡,纯前端)**:`system_prompt_agent` 原先只透传无入口(P57 子目标协议载体,后端真实在用);加 `systemPromptAgent` state + textarea「Agent 提示词(留空用内置)」,submit 传 `trim() || null`;后端零改动(`system_prompt_for` 已处理 None→内置)。
  3. **忘记主机密钥(改放会话右键菜单,非设置页——探码后修正:用户被 TOFU 拒连的第一反应是右键会话,且 host/port 现成免新增 list 命令)**:commands.ts `forgetHostKey(host,port)` 封装(后端 P26-3 命令早已注册但前端零调用);SessionPanel 菜单加「忘记主机密钥」(rdp 会话隐藏);App `forgetHostKey` handler:confirm 说明场景 → 调命令 → alert 反馈"已忘记/无记录/失败"。
  4. **验证(CDP 真机 192.168.79.150)**:界面 tab sec-title=[布局,终端,外观] 且两输入在;**连接会话后改字体保存→`.xterm-screen` 行高 24→16 实时生效**(坑:验 `.xterm` 根元素 computed font-size 恒为 xterm.css 的 16px 不反映 options,须量字符格子);AI tab Agent 提示词 textarea 显示用户现有 P57 提示词;右键菜单五项=连接/断开/**忘记主机密钥**/编辑/删除,点击 confirm 文案正确+真实后端调用成功(alert"已忘记")。
  5. **坑**:a) CDP 测试改 20 验证 20 恒无变化(首轮测试已把 20 存进 config,基线即 20px)——改值断言须避开已存值,且测完恢复配置原值(已 sed 回 14);b) **测试调「忘记主机密钥」真实删掉了 192.168.79.150 的 TOFU pin**(活跃数据,勿删勿动)——测完重连一次自动重新信任恢复 pin,再断开,状态还原;c) 拦截 `window.confirm/alert` 须在页面加载后尽早注入,否则原生对话框会挂住 CDP。
- [x] **P68 终端体验批:WebGL/链接点击/字体缩放/标签键盘导航/右键菜单/多行粘贴警告(已完成,`cargo test` 94 项+Vitest 26 项+CDP 六项断言 ALL PASS)**:
  1. **WebGL 硬件渲染**:`@xterm/addon-webgl`,syncContainers 里 `term.open` 后 try/catch 挂载(上下文超限/驱动问题静默回退 DOM 渲染器);terminals 条目加 `webgl?` 槽位,term.dispose 连带释放。
  2. **链接点击**:`@xterm/addon-web-links` + **后端新命令 `open_external(url)`**(core.rs,`cmd /c start` 空标题占位防注入 + CREATE_NO_WINDOW,**只放行 http/https**;需 `use std::os::windows::process::CommandExt`)。不用 addon 默认 window.open——WebView2 里会导航走应用页面本身。main.rs 注册;commands.ts `openExternal`。
  3. **字体缩放(Ctrl+滚轮/Ctrl+=(-/0))**:TerminalTabs `zoomOffset` $state(临时缩放不持久化,+14/-6 钳制),P67 字体 effect 改读 `termFontSize + zoomOffset`;wheel 监听挂 termArea(`passive:false` 才能 preventDefault 挡 WebView 页面缩放),onMount 注册/清理。
  4. **标签键盘导航**:handleKey 加 Ctrl+Tab(下一个,Shift 反向,循环取模)与 Ctrl+1..9 直达;单标签/越界 no-op。
  5. **终端区右键菜单**:`.term-area oncontextmenu`(仅 `.xterm` 内拦截)+ 复制/粘贴/搜索/清屏 四项(term.clear 含缓冲)+ veil 关闭;样式对齐 SessionPanel ctx-menu。
  6. **多行粘贴警告**:paste() 读剪贴板后按非空行数 >1 时 confirm「粘贴内容包含 N 行命令」——运维安全件,防粘贴块误执行破坏性命令。
  7. **验证(CDP 真机,连接 192.168.79.150)**:WebGL 3 canvas ✓;缩放先 Ctrl+0 归零定基线→Ctrl+= ×2+wheel→canvas 背板尺寸变化→Ctrl+0 精确复位 ✓;右键菜单四项+veil 关闭 ✓;stub 剪贴板两行→Ctrl+V→confirm「2 行命令」✓;单标签 Ctrl+Tab/1 no-throw ✓;`open_external` https 放行真开浏览器/file:// 拒绝 ✓。
  8. **坑**:a) **WebGL 渲染器下所有视觉尺寸被 fit 拉齐区域大小,字号无 DOM 指标**——`.xterm-rows > div` 为 null、`.xterm-screen` 高度恒等于区域、canvas 背板仅重排噪声级变化,断言只能以「重排发生(canvas 尺寸变)+Ctrl+0 精确回到基线」为证(P67 的 DOM 渲染器行高断言在 WebGL 下失效);b) **测试脚本自身会污染被测状态**——探针残留 zoomOffset=7 导致首轮 base 不在零位,断言前必须显式归零;c) clipboard.readText 需文档焦点,CDP 合成事件下直接 stub `navigator.clipboard.readText` 最稳;d) CDP 断言 Svelte DOM 更新须 sleep 等 flush,同步连查恒为旧 DOM。
- [x] **P69 §8.6 泛化验证 + 弱模型输出解析三修复(已完成,真机 glm4:latest 两轮对照 + `cargo test` 98 项全过)**:
  1. **验证方法(§8.6 定稿落地)**:CDP 走真实用户路径(连接 → dock 切 Agent → 输入框提交「检查 sshd 服务报错」) → 每 3s 轮询 DOM(chip/cards/summary)记录全轨迹。**四缺口确认泛化**:计划卡全 step 正确(安全命令自动执行零假确认)、状态 chip 全程正确迁移、退出码判败→模型自动换思路、无新控件需求。容器选择器属 Docker 任务范围(本任务不涉及,P56 已有 dock 入口)。
  2. **实测抓出三个解析缺陷(glm4 不守 P57 输出协议)**:a) **中文反思散文被当命令执行**——「对策：使用 systemctl status...」行含 ASCII 词元(反引号命令)骗过 `is_command_line`,发到 shell 报错退出;b) **同行多 GOAL 零识别**——「GOAL 1. x GOAL 2. y」挤一行,extract_goals 按行前缀匹配得 0 目标;c) **`journalctl -f` 烧满 60s 超时**——follow 类命令永不退出。
  3. **修复(agent.rs)**:a) `is_command_line` 加 **CJK 首字符过滤**(shell 命令必以 ASCII 程序名开头;含中文参数的合法命令如 `echo 你好` 首词元 ASCII 不受影响)+ `strip_enumerator` 剥行首序号("1. 检查磁盘"剥后仍 CJK → 散文;"1. ls -la"剥出真命令,parse_commands 同步应用序号剥除);b) `extract_goals` 加 **goal_segments 同行切段**(按 GOAL 关键字出现位置切,大小写不敏感);`extract_reflexion` 加**中文别名**「对策：/反思：」同等采纳(喂重规划循环);c) 内置两处 agent 提示词加**输出纪律**(GOAL 独占一行/每行至多一条命令/无解释文字)+**禁止 -f/--follow**(须自然退出,看日志用 -n/--since)。
  4. **复测对照(同任务)**:散文执行 3 次→0;`-f` 超时→0(改用 `-n 100 --since`);12 步内 ✓ 任务完成零超时浪费。剩余瑕疵属模型质量(glm4 重复重试一次失败命令、GOAL 不换行),提示词约束兜底,非代码缺陷。
  5. **坑**:a) **用户 config.yaml 的自定义 `system_prompt_agent` 会覆盖内置提示词**——提示词加固必须同步改用户配置(按 P57 先例,已追加输出纪律与禁 -f 两行;config.yaml gitignore 不入库);b) 空命令分支的 CommandStep 卡展示原始 GOAL 文本是**透明化设计**(消息"模型未给出可执行命令"准确),非解析泄漏——GOAL 已被 extract_goals 正常入库并推进;c) 轮询 DOM 断言 AI 轨迹每 3s 一拍即可,chip/cards/summary 三元组足够还原状态机全程。
- [x] **P70 操作记录器:训练数据采集(用户定,已完成,`cargo test` 102 项 + CDP 端到端落盘验证)**:
  1. **需求与设计**:用户要训练终端操作小模型——只记命令没用,须记「上下文 → 命令 → 结果」三元组。新模块 `recorder.rs`:用户输入按 **Enter 行级组装**(PTY 字节流任意分包累积;ESC 序列跳过防方向键/历史回放污染;退格弹末字符记录修正后最终行;行钳 2000 字符),AI 轨迹记 `ai_task`(环境/目标/初始 pwd)与 `ai_step`(step/command/level/exit_code/pwd/output 截 2000)与 QA 问答对(`qa`,答截 4000)。**JSONL append-only 按天滚动**(config 同目录 `logs/terminal-YYYYMMDD.jsonl`,UTC),记录失败静默忽略(日志器绝不能拖垮主功能)。跳过的危险命令记 `skipped:true`(人类干预信号,训练负样本)。
  2. **接线**:CoreState 加 `recorder: Arc<Recorder>`(构造期从 `ui.recording_enabled` 读开关,dir=config 父目录/logs 并 canonicalize 绝对化);`send_input`/`send_active_input` 双钩子(active 经 `ssh.active_name` 定位);ai_job 加 recorder 参数(Agent 起点记 ai_task、task_exec 后记 ai_step、Reject 记 skipped、QA Done/Error 记 qa);`UiConfig.recording_enabled`(serde default true,旧配置兼容)+ `update_ui_config` 同步 set_enabled 即时生效;`recording_info` 命令回目录(剥 `\\?\` 前缀);main.rs 注册。设置弹窗界面 tab 新「记录」卡(checkbox + 落盘说明)。
  3. **验证**:`cargo test` 102 项(+4:分包行组装/ESC+退格/空行与开关 noop/civil 历法锚点);CDP 端到端:真敲 `echo helm-rec-test-70`→input 行落盘;本机 Ollama QA「1+1等于几」→qa 对落盘(答「1+1等于2」);agent 任务→ai_task+ai_step 全字段落盘。
  4. **实测插曲(记录器首战立功)**:trivial 任务「输出当前所在目录路径」glm4 在 step 0 就 `pwd` 答对,但**不输出 DONE,继续乱逛 49 步**(ls/cat 日志/找 auth.log)直到 max_steps 兜底——记录器完整捕获,正是训练需要的负样本;无效步与命令步交替出现(1,3,5 缺号)所以 `MAX_INVALID_STEPS=2` 不触发(计数被成功步清零),靠 max_steps 收敛,属模型质量问题已有护栏。
  5. **坑**:a) cargo test 并行线程共享同一临时目录会互删文件(测试 A 收尾 remove_dir_all 干掉测试 B 正写的文件→NotFound),每测试独立目录(AtomicU64 序号);b) 硬编码"今天"的日期锚点会过期(UTC 已 rollover 到 29 日),历法断言用可心算的历史锚点(0→1970-01-01/31→02-01/365→1971-01-01);c) dev 模式 config 路径回退 `../config.yaml` 使 parent 为 ".." → logs 目录相对路径随 CWD 漂移,须 canonicalize;d) canonicalize 返回 `\\?\` 扩展前缀路径,展示层剥掉。
- [x] **P71 Docker 交互终端(补 P54 首版缺口,已完成,`cargo build` 零警告 + `cargo test` 102 项全过;容器内交互待 Docker 环境复验)**:
  1. **背景**:P54 首版 Docker 会话只开 AI exec 通道,`connect_session` 显式跳过 open_shell——Docker 会话无交互终端(前端 tab 空壳)。
  2. **改动(纯后端两处)**:a) `ssh.rs open_shell` 加 `container: Option<&str>` 参数——kind=Docker 时 PTY 请求后不 `request_shell`,改 `channel.exec("docker exec -it '<容器>' sh")`(容器名单引号转义防 shell 元字符注入;容器名缺失/空白报"Docker 会话未配置容器名";sh 而非 bash 因容器内不一定有 bash);OSC7 注入门控 `kind == Linux` 天然跳过 Docker(容器内 sh 不支持 PROMPT_COMMAND,前端 cd 联动对容器本就不适用——文件面板是宿主 SFTP);b) `core.rs connect_session` 删除 Docker 跳过分支,透传 `info.container`;live 测试 `open_live` 调用点同步。
  3. **前端零改动**:App 只门控 rdp,Docker 会话本就按普通 SSH 建标签/路由输入;SessionForm 已有容器名字段(docker 会话强制填);SysMonitor 对 Docker 显示宿主统计(exec 通道在宿主,可接受);AI Agent 的 docker_exec_cmd 分支(P54)不受影响。
  4. **验证与限制**:本机唯一服务器 192.168.79.150 **无 docker**(agent 实测 `docker: 未找到命令`,连 apt-get 都无——记录器完整捕获了弱模型"尝试安装 docker"的发散轨迹,ai_stop 兜住)。已验证:错误路径(无 docker 宿主上建 docker 会话连接→exec 请求发出→docker 报错文本进终端缓冲+通道 EOF→poller 心跳转 Disconnected,标签仍建)+ 输入路由(dock-test 会话敲命令→recorder 正确记录)。**容器内真实交互(debian/ubuntu 容器 sh 提示符/resize/exit)待有 Docker 环境时复验**。
  5. **坑**:a) docker exec 失败在 SSH 协议层是"成功"(exec 被接受),错误经通道数据+EOF 才暴露——连接瞬时绿点后转断开,属诚实表现未做连接时探测(等待探测会拖慢所有 docker 连接);b) ai_set_mode 在 AI busy 时拒绝("请先停止当前 AI 任务"),CDP 切模式前须确认任务已结束。

## 6. 命令与验证
- 前端开发:`npm run dev`(Vite)
- 全栈开发:`cargo tauri dev`
- 打包:`cargo tauri build`(release exe,src-tauri 目录下)
- Rust 测试:`cargo test`(迁移后旧终端测试消失,SSH 集成测试连 192.168.79.150 需服务器在线否则跳过/失败)
- 冒烟流程:无远程 SSH 时,用本地 cmd/powershell 作为"会话"验证终端链路(连接/交互/多标签/resize)

## 7. 环境事实与坑位记录(随开发追加)
- **环境**:Windows,Node v22.14.0,npm 10.9.2,cargo/rustc 1.95.0;WebView2 Win10/11 自带
- **工具链**:本机**无 MSVC Build Tools**,用 MinGW GCC 15.2.0(`F:\mingw-20.0\MinGW`)GNU 链开发。`rustup` 已装 msvc 与 gnu 两个 toolchain,默认 msvc,但 `F:\Helm` 目录覆盖用 gnu。Tauri 官方只支持 MSVC,但 GNU 链**可行**(官方 issue #9251 确认装好 binutils 即可)。
- **链接器坑(GNU 链必须)**:
  - `cargo install` 不会读项目 `.cargo/config.toml` 的 linker。安装 tauri-cli 需设环境变量:
    `$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="F:/mingw-20.0/MinGW/bin/gcc.exe"`(否则默认找 PATH 里不存在的 `x86_64-w64-mingw32-gcc`,报 `cannot find -lktmw32`)。
  - 项目内 `cargo build`/`cargo tauri dev` **会**读 `.cargo/config.toml`(linker 已指向 gcc.exe)。
  - **`export ordinal too large` 错误**:Tauri 官方模板是 lib+bin 结构(带 cdylib),GNU ld 链接 DLL 时自动导出全部符号,`windows` crate 的导入库符号序号 >65535 触发。**解法:去掉 `[lib]`/`crate-type` 的 cdylib,改纯 bin**(`src-tauri/src/main.rs` 直接 `tauri::Builder` + `generate_context!`)。本应用只做桌面,不需要 cdylib。
- **最终用户运行时**:WebView2 Win10(1803+)/Win11 自带,无需环境。**实测 release exe 的 GNU runtime 已静态链接**(objdump 只看到系统 DLL + WebView2Loader.dll),独立运行无需带 `libgcc_s_seh-1.dll`/`libwinpthread-1.dll`(与早期预判不同,已证伪)。MSVC 链无此问题。
- **Tauri setup 里不能直接 `tokio::spawn`**:`setup` 回调不在 tokio runtime 上,直接 spawn 会 panic("there is no reactor running")。**必须用 `tauri::async_runtime::spawn`**(core.rs 的 `spawn_output_poller` 即用此法)。
- **dev 模式 CWD 是 `src-tauri/`**:`cargo tauri dev` 启动后进程 CWD 为 src-tauri,`./config.yaml` 找不到 → main.rs 加载失败时回退 `../config.yaml`(源码根)。tauri.conf 的 `frontendDist: ../dist`、devUrl 同理按 src-tauri 相对。
- **冒烟验证窗口**:`cargo tauri dev` 阻塞式,用 `Start-Process cmd.exe /c "npm run tauri dev > log 2> err"` 后台起,然后轮询 `Get-Process | Where MainWindowTitle -eq "Helm" -and MainWindowHandle -ne 0`(窗口标题 "Helm",尺寸 1214x838)。注意 `npm run tauri dev` 会 spawn cargo+vite 两个子进程,别误判退出的只是 cmd 包装壳。
- **产物布局**:release 三件套 = `target\release\helm.exe`(~12MB)+ `bundle\msi\Helm_*.msi` + `bundle\nsis\Helm_*-setup.exe`;bundle 自带 WebView2Loader.dll。
- **分发策略(2026-08-28 用户定)**:**不发布预编译安装包**——exe/msi/nsis 三件套不作为发布物,想要安装包的用户拉源码自行 `cargo tauri build`(README 已是源码编译导向,无需改)。后续迭代不再建议打包/发布节点,除非用户主动要求。
- **开发命令注意**:`cargo tauri dev` 是阻塞式(窗口开着不退出)。自动化验证时用 `Start-Process` 后台启动 + `Get-Process`/窗口枚举确认,勿用长超时阻塞。
- **进程坑**:超时强杀 `cargo tauri dev` 会留孤儿 `helm.exe`(受工具会话 Job Object 保护,`Stop-Process`/`taskkill`/提权 TerminateProcess 均拒绝,只能重启机器)。其无窗口且占 `helm.exe` 名,但不影响后续编译运行(新实例 PID 不同)。
- 配置样例 `config.yaml`:1 个会话 `1` → 192.168.79.150:22 root/000000;AI deepseek-chat,DEEPSEEK_API_KEY 环境变量
- 旧根 `Cargo.toml`(ratatui 版)已随 Phase 2 删除,其依赖并入 `src-tauri/Cargo.toml`
- 新版 `src-tauri/Cargo.toml`(Phase 2):tauri v2 + serde + serde_json + tauri-build + russh/russh-keys/tokio/reqwest/futures-util/serde_yaml/regex/anyhow/dirs/async-trait;rustc 1.95 自动解析到 tauri 2.11.5;**P13 新增** `windows = "0.61"`(Win32_Security_Cryptography,DPAPI,入 `[target.'cfg(windows)'.dependencies]`)与 `data-encoding = "2"`(base64),详见 §7 P13;**P26-5 新增** `russh-sftp = "2.4.0"`;GNU 链下 `cargo build`/`cargo test` 均在 src-tauri 目录执行
- **Tauri v2 commands 坑(Phase 2)**:
  - **async command 带 `State<'_, T>` 引用入参必须返回 `Result`**(`AsyncCommandMustReturnResult` trait),否则编译失败。所有 command 统一返回 `Result<T, String>`。
  - **`State<'_, T>` 只实现 `Deref`(返回 `&T`),无 `DerefMut`**。可变字段必须用 `Mutex` 包一层(tokio::sync::Mutex)。本项目中 `config` 字段即用 `Mutex<HelmConfig>`。
  - **tokio::sync::Mutex 不实现 Clone**,跨任务共享需 `Arc<Mutex<...>>`(如 `ai_ctl`、`ssh`、`agent` 均用 Arc)。
  - **命令里不要用 `tokio::runtime::Handle::current().block_on(...)`**:Tauri async command 本身跑在 tokio runtime 上,`block_on` 会 panic。改为直接 `.lock().await` 或调 async 方法。
  - **后台任务取控制通道**:AI 确认用每任务重建的 `mpsc::unbounded_channel`,发送端存 `state.ai_ctl`(Arc<Mutex<Option<Tx>>>),接收端 move 进 spawn 任务。
- **事件约定(core.rs emit)**:`connection`(Connected/Failed/**Disconnected**)、`ai`(StepBegin/Streaming/StepOutputEnd/CommandStep/Done/Error/PendingCommand/Busy)、`terminal-output`({name,data})。前端 Phase 3 按此监听。
  - `Disconnected` 由 `spawn_output_poller` 检测:轮询时记录各会话上一轮 Connected 状态,翻转为断开即 emit(远端关闭/网络中断同步圆点状态)。
- **capabilities(Phase 6 P1)**:`src-tauri/capabilities/default.json` 声明 `core:default` 权限。此前自动生成的 capabilities.json 为空 `{}` 导致 `listen()` 事件全哑 —— 所有事件推送依赖此文件。
- **QA 停止(Phase 6 P3)**:core.rs QA 分支用 `tokio::pin!(chat_fut)` + `tokio::select!` 监听 `ctl_rx`,收到 `AiControl::Cancel` 即 drop chat_fut 中断 HTTP 流并 emit `Done{message:"已停止"}`。
- **update_session 改名语义(Phase 6 P6)**:修改会话名时若旧名处于连接状态,先 `disconnect(old_name)` 再改名,避免后端残留旧名会话。
- **P8 键拦截**:TerminalTabs 的 `handleKey` 以 **capture 阶段** 监听 `window.keydown`(在 xterm textarea 处理前拦截)。Ctrl+C 有选区 → preventDefault+stopPropagation 只做复制;无选区放行给 xterm 发 SIGINT。
- **P13 DPAPI 坑**:
  - `windows` crate 直接加到 `[target.'cfg(windows)'.dependencies]`(`windows = { version = "0.61", features = ["Win32_Security_Cryptography", "Win32_Foundation"] }`),版本与 tauri 传递依赖对齐(0.61.3,复用无需新编译);base64 用 `data-encoding = "2"`(lock 2.11.1,russh-keys 已传递引入,零新增下载)。
  - **`HLOCAL` 是 tuple struct**:`LocalFree(Some(HLOCAL(out_blob.pbData as *mut _)))`,直接 `as _` 会 E0605;`CRYPT_INTEGER_BLOB { cbData, pbData }`,flag 用 `CRYPTPROTECT_UI_FORBIDDEN`(0x1)。
  - **密文永不下发前端**:`get_ai_config` 把 `api_key` 字段替换为哨兵 `Some("·")`(有 key 时),前端据此显示 placeholder"已保存(留空则不修改)";`update_ai_config` 收到 `""` 或 `"·"` 即保留旧密文,仅明文才重新加密。
  - DPAPI 是**用户级**加密:同一 Windows 用户下加密/解密一致,换用户/机器无法解密(属预期安全特性)。
- **P14 布局坑**:
  - **OS 截图 vs WebView 逻辑坐标**:窗口 1214x838(物理)≈ WebView 1200x800(CSS)@DPI 125%,像素截图会有缩放偏移,易误判布局。**用 CDP 更可靠**:`--remote-debugging-port=9229` 注入环境变量 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 再 `Start-Process npm run tauri dev`,`Invoke-RestMethod http://127.0.0.1:9229/json` 拿 webSocketDebuggerUrl,WebSocket 发 `Runtime.evaluate` 取 `getBoundingClientRect()`/`getComputedStyle`。
  - rail 是 `aside.panel`(52px),flyout 绝对定位 `left: calc(100% + 2px)` 宽 230px;CDP 模拟 hover 用 `dispatchEvent(new MouseEvent('mouseenter'))`(Svelte 5 的 `onmouseenter` 可被冒泡事件触发)。
  - 聊天折叠按钮在 ChatPanel 头部,折叠后仅剩 header 行(`flex: 0 0 auto`),`.chat-list` 隐藏。
- **P15 三栏坑**:
  - **`width` vs `flex-basis`**:会话栏/聊天栏宽度用 `width: var(--sp/--cp)` + `.main-row > aside.panel { flex: 0 0 auto }`(App 只把 `--sp/--cp` 用 `style:` 注入到 `main.app`);折叠态在组件内 `.panel.collapsed { width: 44px/36px }` 覆盖。若在 App 里用 `flex: 0 0 var(--sp)` 控宽,折叠态 width 会被 flex-basis 覆盖,导致折叠失效。
  - **折叠后 header 布局**:聊天栏折叠为 36px 竖条时 header 改 `flex-direction: column`,mode-switch 与清空按钮用 `{#if !collapsed}` 隐藏,仅留折叠键。
  - **CDP 交互验证用 `document.querySelectorAll('.main-row > aside.panel')`** 两个面板都是 `aside.panel`,用 first/last-child 区分;Svelte 组件根元素 className 是 scoped hash(`panel s-xxx`),判断 `classList.contains('collapsed')` 即可。
  - **CDP screenshot 的 base64 过大易触发 JSON 解析异常**:WS 收消息时按 `EndOfMessage` 分段累积,整包写入文件后 `ConvertFrom-Json` 或正则 `"data":"([A-Za-z0-9+/=]+)"` 提取,勿直接 `ConvertFrom-Json` 超长字符串。
- **P16 监控坑**:
  - **`Instant` 无 `Default`**:`SysPrev` 若 `#[derive(Default)]` 会编译失败(`std::time::Instant: Default` 不满足),须手动 `impl Default`(`last_time: Instant::now()`)。
  - **parse_netdev 的 tx 在字段索引 8**:测试数据每行须 ≥9 个数字字段(16 个是 /proc/net/dev 真实列数),否则 `fields.get(8)` 为 None 导致 rx/tx 误判为 0、断言失败。
  - **事件注入验证**:Tauri v2 `withGlobalTauri` 未开启时 `window.__TAURI__` 不存在,但 `window.__TAURI_INTERNALS__` 始终在,可用 `window.__TAURI_INTERNALS__.invoke('plugin:event|emit', {event:'sysmon', payload})` 从前端注入假事件走完整条 emit→listen→render 链路(比直接改 DOM 更真实)。
  - **采集失败路径**:`run_exec` 在非 Linux 或命令失败时返回空/错误 → `parse_sample` 返回 None → payload `ok:false` + error,前端显示 `—`+错误文案;SysMonitor 对 `p.name !== activeTab` 的事件直接忽略(切标签清空数据,新会话数据到达前不显示)。
  - 监控是**轮询式**,非事件触发:2s tick 下即使 UI 无监听也持续 exec,payload 里不塞 `data`(Uint8Array),避免与 terminal-output 类型混淆。
- **P17 布局/文件/悬浮窗坑**:
  - **P16 的 `SysMonitor.svelte` 曾于 P17 暂时移除**:P17 把监控信息并入 StatusBar(`onSysMon` 订阅,`CPU xx% · MEM x.xG/y.yG · ↑/↓网速` 小字);**P18 已重建 `SysMonitor.svelte` 薄条(~23px)插回 TerminalTabs 的 `.tabbar` 与 `.term-area` 之间并生效至今**,StatusBar 不再订阅 sysmon。现状以 §4 为准。
  - **`split_whitespace` 解析 ls 行**:不能用 `split(' ')`/`splitn`,ls 列对齐会产生连续空格导致空字段;用 `split_whitespace` 跳过空白,首字段校验 perms 首字符 `-d/l`。
  - **`rm -rf -- /` 过不了 `check_danger` 的 `rm\s+-rf\s+/` 正则**(`--` 打断匹配)→ `remove` 必须加 `assert_removable`(原始路径 trim 后为空或 `/` 直接拒绝),与 check_danger 双保险。
  - **shell 路径转义用 `$(printf %q '...')`**:单引号内 `'` 需替换为 `'\''`;不要手拼引号(空格/通配符易注入)。
  - **Svelte 5 事件里改 `$state` 后 DOM 更新是异步的**:CDP 里 `click()` 后立刻读 `classList` 拿到的是旧值,须 `await setTimeout(~200ms)` 再断言(折叠 32px/展开 180px 即如此验证)。
  - **AiFloat 悬浮窗**:`position:fixed`,header `onmousedown` 记录 `e.clientX - pos.x` 偏移,mousemove/mouseup 挂在 `window`(只在 open 时 add/remove,避免泄漏);右下角 `.ai-resize` 16x16 `nwse-resize` 缩放,最小 260x320;气泡右下 `bottom:44px` 避开状态栏。
  - **AiFloat 的 mode 是单向 prop**:旧 ChatPanel 用 `bind:mode`,新悬浮窗改 `onModeChange` 回调里 `aiMode = m` 同步 App 状态再 `api.aiSetMode`。
  - **文件面板高度用 `style:height` 绑定 + `class:collapsed`**:折叠时 32px,展开 180px;`.fs-resize` 是面板顶部 4px 条(非底部),`startResize` 用 `startH + (startY - ev.clientY)` 上拖变大。
  - **上传 base64 分块 64KB**:`echo '<b64>' | base64 -d >> path`,首块 `>` 后续 `>>`;下载 `base64 -w0`(单行无换行),前端 `atob` 解码成 Uint8Array 存 Blob。
  - **`fs_cwd` 记在 core.rs**:`Mutex<HashMap<String,String>>`,list_dir 里 `cd <path> && pwd` 首行回写;前端双击进目录时拼 `cwd + "/" + name` 传绝对路径。
- **P18 反馈修复坑**:
  - **Svelte 5 拖拽监听挂载时机**:`$effect` 里按 `open` 条件挂 window 的 mousemove/mouseup,拖拽气泡时 `open` 仍为 false → 改条件为 `open || bubbleDragging`;但 CDP 用 `dispatchEvent` 同步发 mousedown+mousemove 时,$effect 尚未 flush 监听 → **验证须 `setTimeout` 分隔**再 dispatch(先 mousedown → 100ms → mousemove → 50ms → mouseup),同步连发拿不到新位置。
  - **run_exec 持有 handle 所有权**:`read_file` 连续两次 exec(读内容 + `head -c N+1|wc -c` 探测截断)须 `run_exec(handle.clone(), ...)`,直接复传会 E0382 moved value;`handle` 是 `Arc<russh Handle>` 非 Copy。
  - **fs_read_file 二进制判定**:exec 输出转 String 是 lossy,二进制文件的 `\0` 字节保留为 U+0000 → `out.contains('\u{0}')` 判二进制;文本预览正常。
  - **cd 跟随正则**:`/(?:^|#|\$|>|;)\s*cd(?:\s+([^\s;|&]+))?/` 行首或提示符(`#`/`$`/`>`/`;`)后匹配 `cd` 目标,`[^\s;|&]+` 截断 `&&`/`;`/管道链;`cd -` 单独跳过。已验证 `grep foo /etc/passwd`(含 `#` 于注释)与 `not a cd here` 不误判。
  - **下载按钮 lastFile 状态**:底部 `⬇ 下载` 按钮 `disabled={!lastFile}`,lastFile 在行 `onclick`(`selectEntry`)与右键(`openContext`)时记录,`{e.name===lastFile?.name && !e.is_dir}` 高亮选中行;下载动作复用 `doDownload`(base64→Blob→`a.click()`)。
- **P19 输入层 cd / 拖拽防开窗 / 预览保存坑**:
  - **CDP 模拟真实键盘输入**:给 xterm 的 `.xterm-helper-textarea` 设 value/`Input.insertText`/execCommand 均**不会**触发 xterm `onData`(值累积但终端不回声);`Input.dispatchKeyEvent` 带 `text` 字段(逐字符 keyDown+keyUp)才会走 `onData` → 远端。整行命令如 `cd /opt` 须逐字符发 + 回车(每字符间隔 30ms)。
  - **残留输入污染**:此前失败的模拟(insertText 等)会在 textarea 遗留值,换用 dispatchKeyEvent 后首行会变成 `ccd /opt` 之类 → 断言前先用原型 setter 清空 textarea(`Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(ta,'')` + input 事件)。
  - **`cd ~` 必须走 `$HOME` 展开**:Rust `shell_quote("~")` → `$(printf %q '~')` → bash 得到字面 `'~'` 不展开 → `cd: ~: 没有那个文件或目录`。新增 `shell_cd_target()`:保持首字符 `~`/`~user` 不引号(展开)+ 剩余部分 `shell_quote`(防注入),如 `~/foo` → `~/$(printf %q 'foo')`。list_dir/mkdir/rename/remove/upload/download/read_file 全部换用。
  - **`current_dir` 优先级**:exec 通道每次新建、pwd 恒为 home,refresh 后直接 `pwd` 会覆盖 cd 目标 → 改为**先读 cwd_map 已跟踪目录**(list_dir 更新),缺失才 exec `pwd` 兜底。
  - **Svelte 5 拖拽验证事件间隔**:bubble mousedown 置 `bubbleDragging=true` → `$effect` 挂 window mousemove/mouseup;CDP 同步连发 mousedown+mousemove 时监听尚未挂 → 必须 mousedown →(≥300ms)→ mousemove →(≥300ms)→ mouseup,否则气泡不动(但 click 仍会因 bubbleMoved 吞掉开窗,行为正确只是位置没变,易误判失败)。
  - **AI 窗按钮索引**:`.ai-window` 内 button 顺序为 QA / Agent / 🗑(清空)/ ×(关闭)/ 发送;CDP 点"关闭"须用 index 3(第 3 个 `icon-btn`),index 2 是清空对话。气泡在窗打开时不可见(`.ai-bubble` 为 null),须先关窗再测气泡。
  - **预览保存复用 fsUpload**:`savePreview` = TextEncoder→`String.fromCharCode`→`btoa`→`fsUpload(name,path,b64,append=false)` 覆盖写;truncated 时按钮禁用。验证:`fs_read_file` 回读内容已改写 + `.pv-saved` 提示出现。
- **P20 面板→终端同步 / is_dir 坑**:
  - **`sendInput` 与 `cdReq` 两条联动路径不要互走**:面板点目录用 `api.sendInput` 直通后端(写 PTY,远端执行并回声);终端敲 `cd` 用 `onCd→cdReq→refresh`。若面板也走 `cdReq`,会再次被 FileBrowser 的 `$effect` 消费造成二次 refresh(虽不循环,但冗余)。
  - **Rust serde rename_all 与前端字段名必须对齐**:`#[serde(rename_all="camelCase")]` 使 `is_dir` 序列化为 `isDir`;前端 TS interface 字段名与 Rust **struct 字段名**一致即可(JS 读的是 JSON key,即 camelCase)。凡是遇到"类型上存在但永远 undefined"的字段,先查 serde。
  - **PowerShell 重写文件会搞坏 UTF-8**:`(Get-Content -Raw) -replace x | Set-Content` 默认 ANSI 编码,中文/emoji 全变 `�?` 且**不可逆**。Svelte 等 UTF-8 源文件只能通过 Edit/Write 工具修改;需要批替换时用 `-Encoding UTF8` 仍不宜(replace 前后有风险),或先用 Read + 整文件重写。
- **P25 Agent / AI 内嵌坑**:详见 §5 P25 第 5 条(a) Svelte 5 props 简写名不符 → 运行时方法缺失;b) vite HMR 全量 reload 清前端会话状态,需 disconnect+connect 重建,teardown 期旧模块 `unsub is not a function` 属噪音;c) 无 API key → Agent::default() 回退走 ensure_ready 报错链路就是验证手段;d) 假 AI 事件经 `window.__TAURI_INTERNALS__.invoke('plugin:event|emit',{event:'ai',payload:{kind:'...'}})` 注入)。
  - **`run_task_exec` 超时序**:`tokio::select!` 超时分支返回 Err,读循环在超时限内按阻塞通道 try 读;`exec(true, full)` 必须传 str 引用(`full.as_str()`),传 `&String` 不会自动 deref 到需要的类型。
  - **cwd 转义只有单引号一种**:`task_exec_cmd` 的 `cd '<cwd>'` 对 `'` 用 `'\''` 闭合,与 `shell_quote` 的 `$(printf %q '...')` 不同(printf %q 会对换行/空白再转义,组装进 shell 命令行时语义等价但可读性差)。
  - **EventPayload kind 字段**:后端 `AiPayload` 序列化后 tag 是 **`kind`**(非 `type`),P25 前脚本里统一 `payload.kind === 'commandStep'`;前端 AiPayload 判别按 `kind`。
  - **回显对话流**:`aiEcho` 用 `pushEcho(name,text) → seq++` 保序,TerminalTabs `$effect` 里 `term.write`;QA streaming 以**首 chunk 前缀 `[AI]`** 引导(后续续写不重复加前缀);commandStep 用 ANSI 转义码着色(`\x1b[36m` 青 / `\x1b[31m` 红 / `\x1b[32m` 绿 / `\x1b[0m` 复位)。
- **Svelte 5 a11y / 构建警告坑(P27)**:
  - **`svelte-ignore` 多码逗号分隔(runes 模式)**:`<!-- svelte-ignore code1, code2 -->`,解析器(extract_svelte_ignore.js)按 `([\w$-]+)(,)?` 循环、遇无逗号码即 break,空格分隔只生效第一个。单码忽略无此问题。
  - **非交互元素不能挂交互 role**:`<li role="button">` → `a11y_no_noninteractive_element_to_interactive_role`;`<div role="button">` 合法。交互行首选 div + `role="button"` + `tabindex="0"` + `onkeydown`(Enter/空格触发同动作,`e.preventDefault()` 防空格滚动)。
  - **`state_referenced_locally`(仅提示,非错误)**:`$state(prop.x)` 初始化只取初值——弹窗 `{#if}` 重挂载场景语义正确,属预期;Svelte 无表达式级豁免,保留即可,勿为此改 `$derived`。
  - **label 关联**:`<label>文本</label><input/>` 报 `a11y_label_has_associated_control`,改包裹式 `<label>文本<input/></label>`(隐式关联)最省事,与 SettingsModal 既有写法一致。

## 8. AI 协作层设计(冻结稿)

> 本节是对 Helm「终端原生 AI 协作层」设计方案的**权威记录与版本锁定**。任何涉及 AI 交互的改动/实现,一律先对照本节的"状态机 → 控件 → 缺口",不得因现状分栏 UI 而偏离。
> 提出时间 2026-08-28。修订前须先更新本节并说明变更理由。

### 8.1 核心命题
- 一句话:**AI 不拥有自己的"地盘",它活在终端的坐标系里。** 每一刻 AI 都处于「某个会话 · 某个 cwd · 某个任务」这条链上,而不是一个悬浮的聊天框。
- QA(解释)与 Agent(执行)不是两种产品,而是同一个"协作者"的**解释模式 / 执行模式**,共用同一个状态机。

### 8.2 唯一状态机(全应用只有一个,所有面板只订阅它)
```
空闲 → 解析意图 → 计划确认 → 执行 → 回读 →(循环)空闲
```
- 空闲态:提示符旁一个光点可唤起,无常驻聊天栏。
- 解析意图态:「意图条」就地浮现,解析结果=目标会话·目录·命令(可选容器),可直接改。
- 计划确认态:「计划卡」展示每条命令+风险标记,可 确认 / 修改 / 丢弃。
- 执行态:命令以卡片就地铺进终端命令流,实时输出可见,可 停止。
- 回读态:退出码 + 摘要,成败一目了然,可 再下一步(回到解析/执行) 或 归位(回到空闲)。
- 危险命令确认 / 推理黄卡 / 退出码回喂,都是该状态机某一态下的**渲染**,而非独立 UI。
- 并发:一次会话可存在多个执行体,以 `AiPayload.name` 区分;各面板只订阅对应 `name` 的状态。**2026-08-28 决议:本期按"每会话单执行体 + 跨会话 name 隔离"落地,同会话多执行体暂缓**(`AiSlot.busy` CAS 保持同会话单任务;`name` 现等价于会话名)。若未来开放同会话多执行体,name 需扩展为"会话×任务"。

### 8.3 三条设计轴
- **状态轴(引擎)**:8.2 的唯一状态机。
- **空间轴**:不建常驻聊天栏;AI 的意图/计划/推理在**上下文帧**随需展开,执行结果以卡片就地入终端流。
- **输入轴**:所有"给 AI 的话"从同一入口进(命令面板 / Alt+I / `!` 增强),与打字共用入口。

### 8.4 设计方法(防"不知道怎么做设计")
1. 挑一个真实任务,从一句话触发开始逐瞬间走一遍,不许跳步。
2. 每个瞬间回答同一组 5 问:当前状态 / 用户看到什么 / AI 给什么信息 / 用户能做什么 / 下一步去哪。
3. 把所有瞬间"用户能做什么"合并:重复项=通用控件,不重复项=该任务特有的边缘控件。

### 8.5 对照现状的落地缺口(走 Docker 任务得出)
真正需要新增/改动仅 4 处,其余全部复用现有零件:
1. 新增 `AiPayload` 事件 `plan/confirmation`(现仅有 `commandStep`/`pendingCommand`)。
2. 计划卡要能「修改」(现 `pendingCommand` 仅确认/拒绝,不可编辑)。
3. 意图条要把目标解析成「会话 → 容器 → 命令」(现只到 会话 → 命令);容器目标对应 `SessionKind::Docker` + `SessionInfo.container`(config.rs 已就绪)。
4. 容器选择器(Docker kind 的边缘控件,承接上一点)。

### 8.6 泛化验证清单
用 8.4 方法再走第二个真实任务(如"帮我搜这台服务报错"),核验 8.5 的 4 个缺口是否仍成立;若出现只属于第二个任务的控件,补充为新增边缘控件。

### 8.7 实现逻辑(后端锚点映射,2026-08-28 冻结)
> 关键认知:后端 `src-tauri/src/ai_job.rs::run_ai_job` 已是这整套循环,`src-tauri/src/core.rs` 定义了 `AiControl`/`AiPayload`/`AiSlot`/`TaskCtx`。实现不是"从零写状态机",而是把隐含在 for 循环顺序里的状态显式化 + 补 8.5 的 4 处缺口。
> 现状循环:emit StepBegin → agent_step(流式 Reasoning+Streaming) → parse_commands(next) → 逐命令 check_danger → 危险 emit PendingCommand、等 ctl_rx 收 Approve/Reject/Cancel → task_exec → emit CommandStep → 输出喂回 agent → 下一轮;DONE/超步 → Done → clear_history/reset_task。
> 现状两处错位:(a) 无显式状态,前端靠 `event.kind` 猜态(违反 8.2"唯一状态机");(b) 计划确认与逐命令确认混在一轮,没有"整份计划卡"。

#### 8.7.1 状态显式化(核心,优先做)
- `core.rs` 加 `pub enum AiRunState { Idle, Parsing, Planning, AwaitingConfirm, Executing, ReadingBack }`;`AiPayload` 加变体 `State { name, state }`。
- `run_ai_job` 用 `set_state()` 辅助,每次换态前 emit `State`;前端 `AiCopilot`/`PromptPanel`/`StatusBar` **只订阅 `kind==="state"`**,其余事件降级为只带内容的 delta payload,不参与判态。
- 状态映射:StepBegin→Parsing;parse_commands 出命令→Planning;首条危险命令 PendingCommand→AwaitingConfirm;task_exec→Executing;CommandStep→ReadingBack;循环回 agent_step→Parsing;Done→Idle。

#### 8.7.2 缺口1:计划卡事件
- `parse_commands` 之后、逐命令确认之前,emit `AiPayload::Planning { name, commands: Vec<{command, level, reason}> }`,给前端整份计划卡。
- 现有 `PendingCommand` 保留:它只"卡住单条确认",与计划卡不冲突。

#### 8.7.3 缺口2:计划可改
- `core.rs::AiControl` 加 `Edit(Vec<String>)`;`ai_control` 命令加 `"edit"` 分支。
- 计划确认态一次性 recv:收 `Edit(cmd)` 用新列表覆盖 commands 再执行;`Reject` 跳过;`Cancel` 停。改动集中在控制通道加一个带负载变体。

#### 8.7.4 缺口3+4:容器定位(后端已就绪,补前端)
- 后端已完成:`core.rs` task 启动时从 `SessionInfo.container` 取 `TaskCtx.container`(core.rs ~524-540);`task_exec` 已把 container 透传给 `run_task_exec`。缺"动态选择"(现在 task 开始时由会话 kind 定死)。
- 前端:`ai_task` 命令加可选 `container: Option<String>`,构建 `TaskCtx` 时覆盖;AiCopilot 对 kind=Docker 会话在意图条/任务启动处加容器下拉。

#### 8.7.5 实施顺序
① 状态显式化(纯增量,四组件统一) → ② 计划卡 + Edit(一次控制通道改动) → ④ 前端容器选择器。改完各跑 `cargo build` / `npm run build` 验证。
