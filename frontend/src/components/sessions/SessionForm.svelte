<script lang="ts">
  import type { SessionInfo } from "../../lib/api";

  interface Props {
    editingSession: SessionInfo | null;
    onCancel: () => void;
    onSubmit: (info: SessionInfo, oldName: string | null) => Promise<string | null>;
  }

  let { editingSession, onCancel, onSubmit } = $props<Props>();

  let kind = $state<"linux" | "windows" | "rdp">(editingSession?.kind ?? "linux");
  let name = $state(editingSession?.name ?? "");
  let host = $state(editingSession?.host ?? "");
  let port = $state(String(editingSession?.port ?? 22));
  let user = $state(editingSession?.user ?? "root");
  let hasSavedPassword = $state(!!editingSession?.password);
  let password = $state("");
  let keyFile = $state(editingSession?.key_file ?? "");
  let error = $state("");
  let saving = $state(false);

  function onKindChange() {
    if (kind === "linux" && !editingSession) {
      port = "22";
      user = "root";
    } else if (kind === "windows" && !editingSession) {
      port = "22";
      user = "Administrator";
    } else if (kind === "rdp" && !editingSession) {
      port = "3389";
      user = "";
    }
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

    <label>平台
      <select bind:value={kind} onchange={onKindChange}>
        <option value="linux">Linux（SSH）</option>
        <option value="windows">Windows（SSH）</option>
        <option value="rdp">远程桌面（RDP）</option>
      </select>
    </label>

    <label>名称
      <input bind:value={name} placeholder="如 prod-web" />
    </label>

    <label>主机
      <input bind:value={host} placeholder="192.168.1.100" />
    </label>

    <div class="row">
      <div>
        <label>端口
          <input bind:value={port} type="number" />
        </label>
      </div>
      <div>
        <label>用户
          <input bind:value={user} placeholder={kind === "windows" ? "Administrator" : "root"} />
        </label>
      </div>
    </div>

    {#if kind === "rdp"}
      <p class="hint">远程桌面连接时由 Windows 系统提示输入密码，无需在此填写。</p>
    {:else}
    <label>密码
      <input
        bind:value={password}
        type="password"
        placeholder={hasSavedPassword ? "已保存（留空则不修改）" : "登录密码"}
      />
    </label>

    <label>私钥文件
      <input bind:value={keyFile} placeholder="C:\Users\me\.ssh\id_rsa（与密码二选一）" />
    </label>
    {/if}

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
    width: 430px;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .modal-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.7rem 1rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  h3 {
    margin: 0;
    font-size: 0.95rem;
  }
  .close {
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-size: 1.15rem;
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
    padding: 0.7rem 1rem 0.9rem;
    overflow-y: auto;
  }
  label {
    display: block;
    font-size: 0.8rem;
    color: var(--fg-muted);
    margin: 0.6rem 0 0.2rem;
  }
  label:first-child {
    margin-top: 0;
  }
  input,
  select {
    width: 100%;
    padding: 0.45rem 0.55rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-size: 0.9rem;
    font-family: inherit;
    background: var(--input-bg);
    color: var(--fg);
    transition: border-color 0.12s ease;
  }
  input:focus,
  select:focus {
    outline: none;
    border-color: var(--accent);
  }
  .hint {
    margin: 0.6rem 0 0;
    font-size: 0.75rem;
    color: var(--fg-muted);
  }
  .row {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.7rem;
  }
  .error {
    color: var(--danger);
    font-size: 0.85rem;
    margin: 0.6rem 0 0;
  }
  .modal-foot {
    display: flex;
    justify-content: flex-end;
    gap: 0.6rem;
    padding: 0.7rem 1rem;
    border-top: 1px solid var(--border);
    flex-shrink: 0;
  }
  button {
    padding: 0.45rem 1.2rem;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--fg);
    cursor: pointer;
    font-size: 0.88rem;
    transition: background 0.12s ease;
  }
  button:hover {
    background: var(--hover);
  }
  button.primary {
    background: var(--accent);
    color: #fff;
    border-color: var(--accent);
    font-weight: 600;
  }
  button.primary:hover {
    background: var(--accent-hover);
  }
  button.ghost:hover {
    background: var(--hover);
  }
</style>
