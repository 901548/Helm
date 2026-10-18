<script lang="ts">
  // P127 四期：还原对话框（预演 → 选择模式 → 确认）
  //
  // 设计铁律（docs/P127-服务器快照-设计方案.md §8）：
  //   - **还原前强制二次快照**（后端硬编码，本 UI 只做告知，不提供关闭开关）
  //   - **默认只预演/暂存**：解到 ~/.helm-snapshots/staged-<ts>/，
  //     用户对比后再决定是否就地覆盖
  //   - `sshd_config` 会被标红警告（还原错误配置可能让 sshd 起不来 = 机器失联）
  import ModalShell from "../ui/ModalShell.svelte";
  import * as api from "../../lib/api";
  import type { DryRunResult } from "../../lib/commands";

  interface Props {
    session: string;
    /** 列表中的快照文件名；后端自动解析其所属链（全量 + 变化包）并叠加 */
    file: string;
    onClose: () => void;
    onRestored: () => void;
  }
  let { session, file, onClose, onRestored }: Props = $props();

  let dry = $state<DryRunResult | null>(null);
  let loading = $state(true);
  let error = $state("");
  let working = $state(false);
  let result = $state<api.RestoreResult | null>(null);
  /** 就地还原需显式勾选（默认只暂存） */
  let inPlace = $state(false);
  /** 就地还原的二次确认输入 */
  let confirmText = $state("");

  async function load() {
    loading = true;
    error = "";
    try {
      dry = await api.snapshotDryRun(session, file);
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    void file;
    load();
  });

  async function doRestore() {
    if (working) return;
    if (inPlace && confirmText !== "RESTORE") {
      error = '就地还原需输入 RESTORE 确认';
      return;
    }
    working = true;
    error = "";
    try {
      result = await api.snapshotRestore(session, file, inPlace);
      onRestored();
    } catch (e) {
      error = String(e);
    } finally {
      working = false;
    }
  }

  function fmtSize(b: number): string {
    if (!b) return "—";
    if (b < 1024) return b + "B";
    if (b < 1024 * 1024) return (b / 1024).toFixed(1) + "KB";
    return (b / 1024 / 1024).toFixed(1) + "MB";
  }
</script>

<ModalShell labelledBy="rs-title" width="680px" height="560px" resizable minWidth={460} minHeight={320} {onClose}>
  <header class="rs-head">
    <h3 id="rs-title">还原快照</h3>
    <button class="rs-close" title="关闭 (Esc)" onclick={onClose}>×</button>
  </header>

  <div class="rs-body">
    <div class="rs-pack">{file}</div>

    {#if loading}
      <p class="rs-hint">正在预演（读取包内容并与当前状态对比）…</p>
    {:else if dry}
      <!-- 预演摘要（§8.3：还原必须可预演） -->
      <div class="rs-summary">
        {#if dry.packs.length > 1}合并 {dry.packs.length} 个包，{/if}包内 <strong>{dry.total}</strong> 个文件：
        <span class="ov">覆盖 {dry.overwrite}</span>
        <span class="cr">新建 {dry.create}</span>
      </div>

      {#if dry.packs.length > 1}
        <!-- 链式还原：全量包 + 变化包按时间正序依次解包叠加 -->
        <div class="rs-chain">
          <div class="rs-chain-title">此快照属于一条链，将按顺序依次解包叠加（后一个包覆盖前一个）：</div>
          {#each dry.packs as p, i (p)}
            <code class="rs-chain-pack">{p}</code>{#if i < dry.packs.length - 1}<span class="rs-chain-arrow">→</span>{/if}
          {/each}
        </div>
      {/if}

      {#if dry.touchesSshdConfig}
        <div class="rs-danger">
          <svg class="rs-danger-ico" viewBox="0 0 16 16" fill="none" aria-hidden="true">
            <path d="M8 2.3 14.4 13.2H1.6L8 2.3Z" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>
            <path d="M8 6.5v3" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
            <circle cx="8" cy="11.5" r="0.8" fill="currentColor"/>
          </svg>
          此快照包含 <code>/etc/ssh/sshd_config</code>。若包内配置有误，还原后
          <code>sshd</code> 可能无法启动 → <strong>机器失联</strong>。
          后端会做 <code>sshd -t</code> 语法校验，失败则不 reload。
        </div>
      {/if}

      <!-- 还原前会强制再拍一次（不可关闭） -->
      <div class="rs-notice">
        还原前会<strong>自动对当前状态再拍一次快照</strong>（不可关闭）——
        否则还原本身就成了不可逆操作。
      </div>

      <div class="rs-mode">
        <label class="rs-radio">
          <input type="radio" bind:group={inPlace} value={false} />
          <span><strong>暂存</strong>（推荐）— 解到独立目录，你对比后再自行覆盖</span>
        </label>
        <label class="rs-radio">
          <input type="radio" bind:group={inPlace} value={true} />
          <span><strong>就地还原</strong> — 直接覆盖原路径（需二次确认）</span>
        </label>
      </div>

      {#if inPlace}
        <div class="rs-confirm">
          <label>
            输入 <code>RESTORE</code> 确认：
            <input bind:value={confirmText} placeholder="RESTORE" />
          </label>
        </div>
      {/if}

      <div class="rs-diff">
        <div class="rs-diff-title">
          变更明细{dry.truncated ? `（仅显示前 ${dry.diff.length} 条）` : ""}
        </div>
        {#each dry.diff as d (d.path)}
          <div class="rs-row">
            <span class="rs-act" class:ov={d.action === "overwrite"} class:cr={d.action === "create"}>
              {d.action === "overwrite" ? "覆盖" : "新建"}
            </span>
            <span class="rs-path">{d.path}</span>
            <span class="rs-sz">
              {#if d.action === "overwrite"}{fmtSize(d.currentSize)} → {/if}{fmtSize(d.packSize)}
            </span>
          </div>
        {/each}
      </div>
    {/if}

    {#if error}<div class="rs-error">{error}</div>{/if}

    {#if result}
      <div class="rs-done">
        <div>✓ 已还原 <strong>{result.restored}</strong> 个文件</div>
        <div class="rs-done-line">还原前快照：<code>{result.preSnapshot}</code></div>
        <div class="rs-done-line">暂存目录：<code>{result.stagedDir}</code></div>
        {#if result.sshdCheck === false}
          <div class="rs-warn">sshd 校验失败（未 reload）：{result.warning}</div>
        {:else if result.sshdCheck === true}
          <div class="rs-ok">sshd -t 校验通过</div>
        {/if}
        {#if result.stagedDir !== "/"}
          <div class="rs-done-line dim">
            确认无误后，可用 <code>cp -a {result.stagedDir}/. /</code> 覆盖（或在文件面板里逐项处理）
          </div>
        {/if}
      </div>
    {/if}
  </div>

  <footer class="rs-foot">
    <button class="rs-btn" onclick={onClose}>关闭</button>
    <button
      class="rs-btn primary"
      class:danger={inPlace}
      onclick={doRestore}
      disabled={working || loading || !dry || (inPlace && confirmText !== "RESTORE")}
    >{working ? "还原中…" : inPlace ? "就地还原" : "解到暂存目录"}</button>
  </footer>
</ModalShell>

<style>
  .rs-head {
    display: flex;
    align-items: center;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .rs-head h3 {
    margin: 0;
    font-size: 0.9rem;
  }
  .rs-close {
    margin-left: auto;
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-size: 1.15rem;
    line-height: 1;
    padding: 0 0.35rem;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .rs-close:hover {
    color: var(--fg);
    background: var(--hover);
  }
  .rs-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0.8rem 1rem;
  }
  .rs-pack {
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.76rem;
    color: var(--fg-muted);
    margin-bottom: 0.5rem;
    word-break: break-all;
  }
  .rs-summary {
    font-size: 0.8rem;
    margin-bottom: 0.5rem;
  }
  .rs-summary .ov {
    color: var(--warning);
    margin-left: 0.4rem;
  }
  .rs-summary .cr {
    color: var(--ok);
    margin-left: 0.4rem;
  }
  /* 链式还原：包序列展示 */
  .rs-chain {
    padding: 0.45rem 0.7rem;
    background: var(--input-bg);
    border-radius: var(--radius-sm);
    font-size: 0.72rem;
    line-height: 1.8;
    margin-bottom: 0.5rem;
    word-break: break-all;
  }
  .rs-chain-title {
    color: var(--fg-muted);
    margin-bottom: 0.15rem;
  }
  .rs-chain-pack {
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.72rem;
  }
  .rs-chain-arrow {
    color: var(--fg-muted);
    margin: 0 0.3rem;
  }
  .rs-danger {
    padding: 0.5rem 0.7rem;
    background: var(--warn-bg);
    border-radius: var(--radius-sm);
    font-size: 0.76rem;
    line-height: 1.6;
    margin-bottom: 0.5rem;
  }
  .rs-danger-ico {
    display: inline-block;
    width: 13px;
    height: 13px;
    vertical-align: -2px;
    margin-right: 0.25rem;
    color: var(--warning);
  }
  .rs-notice {
    padding: 0.5rem 0.7rem;
    background: var(--input-bg);
    border-radius: var(--radius-sm);
    font-size: 0.76rem;
    line-height: 1.6;
    margin-bottom: 0.6rem;
  }
  .rs-mode {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    margin-bottom: 0.6rem;
  }
  .rs-radio {
    display: flex;
    align-items: flex-start;
    gap: 0.4rem;
    font-size: 0.78rem;
    cursor: pointer;
  }
  .rs-radio input {
    margin-top: 0.15rem;
  }
  .rs-confirm {
    margin-bottom: 0.6rem;
    font-size: 0.78rem;
  }
  .rs-confirm input {
    margin-left: 0.4rem;
    padding: 0.2rem 0.4rem;
    background: var(--input-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--fg);
    font-family: inherit;
  }
  .rs-diff {
    border-top: 1px solid var(--border);
    padding-top: 0.4rem;
  }
  .rs-diff-title {
    font-size: 0.74rem;
    color: var(--fg-muted);
    margin-bottom: 0.3rem;
  }
  .rs-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.15rem 0;
    font-size: 0.74rem;
  }
  .rs-act {
    flex-shrink: 0;
    padding: 0 0.3rem;
    border-radius: var(--radius-sm);
    font-size: 0.68rem;
    background: var(--input-bg);
  }
  .rs-act.ov {
    color: var(--warning);
  }
  .rs-act.cr {
    color: var(--ok);
  }
  .rs-path {
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rs-sz {
    margin-left: auto;
    flex-shrink: 0;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .rs-error {
    margin: 0.5rem 0;
    padding: 0.4rem 0.6rem;
    background: var(--warn-bg);
    border-radius: var(--radius-sm);
    font-size: 0.75rem;
    color: var(--danger);
  }
  .rs-done {
    margin-top: 0.6rem;
    padding: 0.5rem 0.7rem;
    background: var(--input-bg);
    border-radius: var(--radius-sm);
    font-size: 0.76rem;
    line-height: 1.7;
  }
  .rs-done-line {
    color: var(--fg-muted);
    word-break: break-all;
  }
  .rs-done-line.dim {
    opacity: 0.85;
  }
  .rs-warn {
    color: var(--warning);
  }
  .rs-ok {
    color: var(--ok);
  }
  .rs-hint {
    font-size: 0.78rem;
    color: var(--fg-muted);
  }
  .rs-foot {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    padding: 0.6rem 1rem;
    border-top: 1px solid var(--border);
    flex-shrink: 0;
  }
  .rs-btn {
    padding: 0.35rem 0.9rem;
    font-size: 0.78rem;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    cursor: pointer;
  }
  .rs-btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
    font-weight: 600;
  }
  .rs-btn.primary.danger {
    background: var(--danger);
    border-color: var(--danger);
  }
  .rs-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
