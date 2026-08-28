<script lang="ts">
  import type { SessionInfo } from "../../lib/api";

  interface Props {
    editingSession: SessionInfo | null;
    onCancel: () => void;
    onSubmit: (info: SessionInfo, oldName: string | null) => Promise<string | null>;
  }

  let { editingSession, onCancel, onSubmit } = $props<Props>();

  let kind = $state<"linux" | "windows" | "rdp" | "docker">(editingSession?.kind ?? "linux");
  let name = $state(editingSession?.name ?? "");
  let host = $state(editingSession?.host ?? "");
  let port = $state(String(editingSession?.port ?? 22));
  let user = $state(editingSession?.user ?? "root");
  let container = $state(editingSession?.container ?? "");
  let hasSavedPassword = $state(!!editingSession?.password);
  let password = $state("");
  let keyFile = $state(editingSession?.key_file ?? "");
  let error = $state("");
  let saving = $state(false);

  function applyKindDefaults() {
    if (editingSession) return;
    const preset: Record<string, { port: string; user: string }> = {
      linux: { port: "22", user: "root" },
      windows: { port: "22", user: "Administrator" },
      rdp: { port: "3389", user: "" },
      docker: { port: "22", user: "root" },
    };
    const p = preset[kind];
    if (p) {
      port = p.port;
      user = p.user;
    }
  }

  function selectKind(k: "linux" | "windows" | "rdp" | "docker") {
    kind = k;
    applyKindDefaults();
  }

  async function submit() {
    error = "";
    if (!name.trim() || !host.trim()) {
      error = "会话名称与主机不能为空";
      return;
    }
    const parsedPort = parseInt(port, 10);
    if (Number.isNaN(parsedPort) || parsedPort < 1 || parsedPort > 65535) {
      error = "端口无效";
      return;
    }
    if (kind === "docker" && !container.trim()) {
      error = "请填写容器名";
      return;
    }
    if (kind !== "rdp" && !password && !keyFile && !hasSavedPassword) {
      error = "请至少填写密码或私钥文件路径";
      return;
    }
    saving = true;
    const info: SessionInfo = {
      name: name.trim(),
      kind,
      host: host.trim(),
      port: parsedPort,
      user: user.trim() || "root",
      password: hasSavedPassword && !password ? "·" : (password ? password : null),
      key_file: keyFile ? keyFile : null,
      container: kind === "docker" ? container.trim() : null,
    };
    const err = await onSubmit(info, editingSession?.name ?? null);
    saving = false;
    if (err) error = err;
  }
</script>

<!-- 遮罩：点击空白取消；modal 容器 stopPropagation 防误关，内部控件均为可访问的按钮/输入框 -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="veil" onclick={onCancel}>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="modal" onclick={(e) => e.stopPropagation()}>
    <header class="modal-head">
      <h3>{editingSession ? "编辑会话" : "新建会话"}</h3>
      <button class="close" title="关闭" onclick={onCancel}>×</button>
    </header>

    <div class="modal-body">
      <!-- 目标主机 -->
      <section class="card">
        <div class="sec-title"><span class="sec-ico"></span>目标主机</div>

        <div class="f-label">平台类型</div>
        <div class="type-grid">
          <button
            type="button"
            class="type-card {kind === 'linux' ? 'on' : ''}"
            onclick={() => selectKind("linux")}
          >
            <span class="tc-main">Linux</span>
            <span class="tc-sub">SSH · bash</span>
          </button>
          <button
            type="button"
            class="type-card {kind === 'windows' ? 'on' : ''}"
            onclick={() => selectKind("windows")}
          >
            <span class="tc-main">Windows</span>
            <span class="tc-sub">SSH · cmd</span>
          </button>
          <button
            type="button"
            class="type-card {kind === 'rdp' ? 'on' : ''}"
            onclick={() => selectKind("rdp")}
          >
            <span class="tc-main">远程桌面</span>
            <span class="tc-sub">RDP</span>
          </button>
          <button
            type="button"
            class="type-card {kind === 'docker' ? 'on' : ''}"
            onclick={() => selectKind("docker")}
          >
            <span class="tc-main">Docker</span>
            <span class="tc-sub">SSH 容器</span>
          </button>
        </div>

        <label class="f-label" for="f-name">会话名称</label>
        <input id="f-name" bind:value={name} placeholder="如 prod-web" />

        <label class="f-label" for="f-host">主机地址</label>
        <input id="f-host" bind:value={host} placeholder="192.168.1.100" />

        <div class="row">
          <div>
            <label class="f-label" for="f-port">端口</label>
            <input id="f-port" bind:value={port} type="number" />
          </div>
          <div>
            <label class="f-label" for="f-user">用户</label>
            <input id="f-user" bind:value={user} placeholder={kind === "windows" ? "Administrator" : "root"} />
          </div>
        </div>

        {#if kind === "docker"}
          <label class="f-label" for="f-container">容器名</label>
          <input id="f-container" bind:value={container} placeholder="如 my-app（远端主机上的容器）" />
          <p class="hint">连接远端主机后，AI 命令会注入该容器执行（docker exec）。</p>
        {/if}
      </section>

      <!-- 认证 -->
      <section class="card">
        <div class="sec-title"><span class="sec-ico"></span>认证</div>

        {#if kind === "rdp"}
          <p class="hint">远程桌面连接时由 Windows 系统提示输入密码，无需在此填写。</p>
        {:else}
          <label class="f-label" for="f-password">密码</label>
          <input
            id="f-password"
            bind:value={password}
            type="password"
            placeholder={hasSavedPassword ? "已保存（留空则不修改）" : "登录密码"}
          />

          <label class="f-label" for="f-key">私钥文件</label>
          <input id="f-key" bind:value={keyFile} placeholder="C:\Users\me\.ssh\id_rsa（与密码二选一）" />
        {/if}
      </section>

      {#if error}
        <p class="error">{error}</p>
      {/if}
    </div>

    <footer class="modal-foot">
      <button class="ghost" onclick={onCancel}>取消</button>
      <button class="primary" onclick={submit} disabled={saving}>
        {saving ? "保存中…" : "保存"}
      </button>
    </footer>
  </div>
</div>

<style>
  .veil {
    position: fixed;
    inset: 0;
    background: rgba(8, 12, 18, 0.5);
    backdrop-filter: blur(2px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
  }
  .modal {
    background: var(--modal-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    width: 480px;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .modal-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.8rem 1.1rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  h3 {
    margin: 0;
    font-size: 0.96rem;
  }
  .close {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-size: 1.2rem;
    cursor: pointer;
    padding: 0 0.35rem;
    border-radius: var(--radius-sm);
    line-height: 1;
  }
  .close:hover {
    color: var(--fg);
    background: var(--hover);
  }
  .modal-body {
    padding: 0.9rem;
    overflow-y: auto;
  }

  .card {
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: 0.9rem 1rem 1rem;
  }
  .card + .card {
    margin-top: 0.8rem;
  }
  .sec-title {
    display: flex;
    align-items: center;
    gap: 0.42rem;
    font-size: 0.82rem;
    font-weight: 600;
    color: var(--fg);
    letter-spacing: 0.03em;
    margin-bottom: 0.6rem;
  }
  .sec-ico {
    width: 8px;
    height: 8px;
    border-radius: 2px;
    background: var(--accent);
    box-shadow: 0 0 6px var(--accent);
  }

  .type-grid {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 0.5rem;
    margin-bottom: 0.2rem;
  }
  .type-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.2rem;
    padding: 0.55rem 0.25rem;
    background: var(--input-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: border-color 0.13s ease, background 0.13s ease, box-shadow 0.13s ease;
  }
  .type-card:hover {
    border-color: var(--accent);
  }
  .type-card.on {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .tc-main {
    font-size: 0.82rem;
    font-weight: 600;
    color: var(--fg);
    line-height: 1.1;
  }
  .tc-sub {
    font-size: 0.62rem;
    color: var(--fg-muted);
  }
  .type-card.on .tc-main,
  .type-card.on .tc-sub {
    color: var(--accent);
  }

  .f-label {
    display: block;
    font-size: 0.78rem;
    color: var(--fg-muted);
    margin: 0.62rem 0 0.22rem;
  }
  input {
    width: 100%;
    padding: 0.48rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-size: 0.9rem;
    font-family: inherit;
    background: var(--input-bg);
    color: var(--fg);
    transition: border-color 0.12s ease;
  }
  input:hover {
    border-color: color-mix(in srgb, var(--border) 60%, var(--accent));
  }
  input:focus {
    outline: none;
    border-color: var(--accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent);
  }
  .hint {
    margin: 0.55rem 0 0;
    font-size: 0.75rem;
    color: var(--fg-muted);
    line-height: 1.45;
  }
  .row {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.7rem;
  }
  .error {
    color: var(--danger);
    font-size: 0.85rem;
    margin: 0.7rem 0 0;
  }
  .modal-foot {
    display: flex;
    justify-content: flex-end;
    gap: 0.6rem;
    padding: 0.75rem 1.1rem;
    border-top: 1px solid var(--border);
    flex-shrink: 0;
  }
  button {
    font-family: inherit;
  }
  .modal-foot button {
    padding: 0.46rem 1.25rem;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--fg);
    cursor: pointer;
    font-size: 0.88rem;
    transition: background 0.12s ease;
  }
  .modal-foot button:hover {
    background: var(--hover);
  }
  .modal-foot button.primary {
    background: var(--accent);
    color: #fff;
    border-color: var(--accent);
    font-weight: 600;
  }
  .modal-foot button.primary:hover {
    background: var(--accent-hover);
  }
  .modal-foot button.ghost:hover {
    background: var(--hover);
  }
</style>