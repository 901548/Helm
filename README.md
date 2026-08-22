# Helm

**带本地 AI Agent 的 SSH 终端工具 —— 你掌舵,AI 执行。**

Windows 桌面应用:多标签 SSH 终端、SFTP 文件管理、系统监控,以及一个能直接在远程服务器上逐步执行命令完成自然语言任务的 AI Agent。基于 Tauri v2(WebView2),Win10/11 双击即用,无需安装浏览器或运行时。

## 功能特性

### 终端与连接
- **多标签 SSH 终端**:每会话一个 xterm.js 实例,流式直通 PTY,字体渲染完整(Cascadia Mono)
- **多平台会话**:Linux SSH / Windows SSH(跳过 bash 专属注入,PowerShell Agent 执行)/ 远程桌面 RDP(生成临时 .rdp 拉起系统 mstsc 控屏,密码不落盘)
- **主机密钥 TOFU**:首次连接记录主机密钥(`known_hosts.json`,与 config.yaml 同目录),变更即拒绝并提示,防中间人
- **连接超时 45s**:兼容慢认证(UseDNS/反向解析)的老服务器

### 文件管理(SFTP 直传)
- 底部文件面板:浏览/新建目录/重命名/删除/上传/下载/文本预览编辑保存
- **面板 ⇄ 终端双向 cd 联动**:终端 `cd` 经服务器权威 PWD(OSC7 标记)同步面板;面板双击目录同步在终端执行 `cd`(仅 Linux 会话)
- 删除有根路径拒绝 + 危险命令判级双保险

### 系统监控
- 终端上方薄条:CPU / 内存 / 负载 / 网速,2 秒刷新,独立 exec 通道不污染终端
- Linux(/proc)与 Windows(PowerShell CIM)双模式采集

### AI 副驾驶(终端内嵌)
- **QA 模式**:流式问答,回答直接回显在终端
- **Agent 模式**:自然语言任务 → AI 逐步生成命令 → 自动在活动会话执行 → 读输出与退出码回喂 → 直到任务完成输出 DONE
  - cwd 随执行结果权威跟踪(`###HELM_PWD###` 标记回传)
  - Linux 走 bash,Windows 走 PowerShell EncodedCommand
- **危险命令确认**:shell 分词 + 语义判级(Critical / Warning / Safe),`rm -rf /`、`mkfs`、fork 炸弹等 Critical 命令强制弹确认(执行/跳过/取消),安全命令免打扰直接执行
- 兼容 OpenAI 风格 API(DeepSeek 等),支持自定义 api_base_url / extra_headers / extra_body

### 界面与安全
- **双主题**:白天 / 黑夜 / 跟随系统,xterm 终端同步换色
- **敏感信息 DPAPI 加密**:AI API Key 与会话密码以 Windows 用户级密文存储,明文永不下发前端
- **窗口尺寸/位置自动记忆**(含最小化离屏守卫)
- 设置持久化到 `config.yaml`(原子写入防损坏)

## 快捷键

| 快捷键 | 功能 |
|---|---|
| `Alt + I` | 终端底部弹出 AI 任务输入行 |
| `Alt + L` | 开关 AI 全屏日志面板 |
| `Ctrl + F` | 终端内搜索 |
| `Ctrl + C` | 有选区时复制(无选区发送 SIGINT) |
| `Ctrl + V` | 粘贴 |
| 鼠标中键 | 关闭标签页 |

## 从源码构建

环境要求:Windows 10/11、Node ≥ 20、Rust(stable)、WebView2(系统自带)。

```bash
npm install          # 安装前端依赖
cargo tauri dev      # 开发模式(热重载)
cargo tauri build    # 打包 release:helm.exe + msi + nsis 安装包
```

产物位置:`src-tauri/target/release/helm.exe`、`src-tauri/target/release/bundle/`。

运行测试:

```bash
cd src-tauri && cargo test    # 61 项(含 SSH/SFTP live 测试,服务器离线自动跳过)
npm run build                 # 前端构建
```

## 配置

配置文件查找顺序:命令行指定 > `./config.yaml` > `~/.config/helm/config.yaml`。

```yaml
sessions:
  - name: my-server
    kind: linux          # linux(默认) | windows | rdp
    host: 192.168.1.100
    port: 22
    user: root
    password: null       # 界面保存后为 DPAPI 密文(enc: 前缀),明文仅作首次输入
    key_file: null       # 私钥路径(~ 展开),密码优先

ai:
  model: deepseek-chat
  api_key_env: DEEPSEEK_API_KEY   # 环境变量回退
  api_key: null                   # 界面直填后存 DPAPI 密文,优先于环境变量
  api_base_url: null              # 自定义 OpenAI 兼容端点
  mode: qa                        # qa | agent
  max_steps: 50                   # Agent 单任务最大步数
  command_timeout_secs: null      # Agent 单命令超时(默认 60s)
  agent_confirm: false            # true = 所有命令都确认;false = 仅危险命令确认
  system_prompt: |
    你是一个专业的运维AI助手……

ui:
  theme: dark           # light | dark | system
  window_width: 1200
  window_height: 800
```

> AI 的 API Key 与会话密码建议经设置弹窗 / 会话表单填写,落盘自动加密,不要手写明文。

## 项目结构

```
F:\Helm\
├── frontend/src/
│   ├── components/     # SessionPanel / SessionForm / TerminalTabs / SysMonitor
│   │                   # FileBrowser / AiCopilot / StatusBar / SettingsModal
│   └── lib/api.ts      # TS 类型 + invoke/事件封装
├── src-tauri/src/
│   ├── main.rs         # Tauri 入口 + 窗口持久化
│   ├── core.rs         # CoreState + commands + AI 任务调度
│   ├── ssh.rs          # SshManager:PTY/exec/SFTP/Agent 任务执行
│   ├── agent.rs        # AI 客户端(QA 流式 + Agent 单步)
│   ├── safety.rs       # 危险命令判级(shell 分词 + 语义)
│   ├── fs.rs           # SFTP 文件命令
│   ├── monitor.rs      # 系统监控采集(Linux/Windows)
│   ├── crypto.rs       # DPAPI 加解密
│   ├── known_hosts.rs  # 主机密钥 TOFU
│   └── config.rs       # 配置加载/原子保存
└── config.yaml         # 运行时配置
```

## 技术栈

- **后端**:Rust + Tauri v2、russh / russh-keys / russh-sftp(SSH 协议)、tokio、reqwest(AI HTTP 流式)、serde_yaml
- **前端**:Svelte 5(runes)+ Vite 6 + xterm.js 5(fit / search 插件)+ TypeScript
- **安全**:Windows DPAPI(密文存储)、TOFU known_hosts、危险命令语义判级

## License

MIT
