<script lang="ts">
  // P127 快照面板（一期：探测 + 手动打包 + 列表 + 删除）
  //
  // 设计要点（docs/P127-服务器快照-设计方案.md）：
  //   - **不让用户选档位**（§4.8）：探测后以「结论」形式告知，而非选择题
  //   - 打包前做体积与空间检查，不够则**拒绝**（§7.1）—— 磁盘写满的连带损害
  //     远大于"这次没拍成"
  //   - 残包不进列表：pack 内部校验失败即删（§14.1）—— 残包比没有包更危险
  import ModalShell from "../ui/ModalShell.svelte";
  import * as api from "../../lib/api";
  import type { SnapshotEntry, SnapshotProbe } from "../../lib/commands";

  interface Props {
    session: string;
    onClose: () => void;
  }
  let { session, onClose }: Props = $props();

  let probe = $state<SnapshotProbe | null>(null);
  let entries = $state<SnapshotEntry[]>([]);
  let loading = $state(true);
  let packing = $state(false);
  let error = $state("");
  let notice = $state("");
  /** P127 二期：命令前拦截开关（默认关，须显式开启） */
  let interceptOn = $state(false);
  /** P127 三期：自动基线开关 */
  let baselineOn = $state(false);
  /** 该会话是否已建立基线 */
  let baselineEst = $state(false);

  /** 用户可勾选的路径（默认用探测推荐值） */
  let picked = $state<Set<string>>(new Set());

  async function load() {
    loading = true;
    error = "";
    try {
      const [p, list, ic, bs] = await Promise.all([
        api.snapshotProbe(session),
        api.snapshotList(session),
        api.snapshotGetIntercept(),
        api.baselineStatus(session),
      ]);
      probe = p;
      entries = list;
      interceptOn = ic;
      baselineOn = bs[0];
      baselineEst = bs[1];
      // 首次加载用探测推荐值预勾选
      if (picked.size === 0) picked = new Set(p.recommended);
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  async function toggleIntercept() {
    const next = !interceptOn;
    try {
      await api.snapshotSetIntercept(next);
      interceptOn = next;
    } catch (e) {
      error = String(e);
    }
  }

  async function toggleBaseline() {
    const next = !baselineOn;
    try {
      await api.baselineSetEnabled(next);
      baselineOn = next;
    } catch (e) {
      error = String(e);
    }
  }

  $effect(() => {
    void session;
    load();
  });

  /** 当前勾选路径的预估体积（MB） */
  const estMb = $derived(
    (probe?.pathSizes ?? [])
      .filter(([p]) => picked.has(p))
      .reduce((s, [, mb]) => s + mb, 0),
  );

  function toggle(p: string) {
    const next = new Set(picked);
    if (next.has(p)) next.delete(p);
    else next.add(p);
    picked = next;
  }

  async function doPack() {
    if (!probe || packing) return;
    const paths = [...picked];
    if (paths.length === 0) {
      error = "请至少选择一个路径";
      return;
    }
    packing = true;
    error = "";
    notice = "";
    try {
      const r = await api.snapshotPack(session, paths, probe.diskAvailMb, estMb);
      notice = `已创建 ${r.file}（${fmtSize(r.size)}，耗时 ${(r.elapsedMs / 1000).toFixed(1)}s）`;
      await api.snapshotPrune(session);
      await load();
    } catch (e) {
      // 空间不足等拒绝原因由后端给出具体数字，直接展示
      error = String(e);
    } finally {
      packing = false;
    }
  }

  async function doRemove(file: string) {
    error = "";
    try {
      await api.snapshotRemove(session, file);
      await load();
    } catch (e) {
      error = String(e);
    }
  }

  function fmtSize(b: number): string {
    if (b < 1024) return b + "B";
    if (b < 1024 * 1024) return (b / 1024).toFixed(1) + "KB";
    return (b / 1024 / 1024).toFixed(1) + "MB";
  }

  function fmtTime(ts: number): string {
    if (!ts) return "—";
    const d = new Date(ts * 1000);
    const p = (n: number) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
  }
</script>

<ModalShell labelledBy="snap-title" width="620px" {onClose}>
  <header class="snap-head">
    <h3 id="snap-title">服务器快照</h3>
    <button class="snap-close" title="关闭 (Esc)" onclick={onClose}>×</button>
  </header>

  <div class="snap-body">
    {#if loading}
      <p class="snap-hint">探测中…</p>
    {:else if probe}
      <!-- 探测结论：以「已按本机情况配置」呈现，而非让用户选档位（设计稿 §4.8） -->
      <div class="snap-conclusion">{probe.reason}</div>

      <div class="snap-caps">
        <span>磁盘可用 {probe.diskAvailMb}MB</span>
        {#if probe.lvmFreeMb > 0}<span class="ok">LVM 剩余 {probe.lvmFreeMb}MB</span>{/if}
        {#if probe.reflink}<span class="ok">支持 reflink</span>{/if}
        {#if !probe.reflink && probe.lvmFreeMb === 0}
          <span class="dim">无块级快照能力（用 tar 打包）</span>
        {/if}
      </div>

      <!-- P127 二期：命令前拦截。默认关 —— 会拦输入，须显式开启 -->
      <label class="snap-item snap-toggle">
        <input type="checkbox" checked={interceptOn} onchange={toggleIntercept} />
        <span>危险命令前拦截（执行 <code>rm -rf /</code> 这类不可逆命令前弹确认）</span>
      </label>
      {#if interceptOn}
        <div class="snap-hint-sm">
          仅拦 <strong>Critical</strong> 级（系统级不可逆）；<code>rm -rf /etc/xxx</code> 等 Warning 不拦。
          <code>↑</code> 历史回放与 Tab 补全无法追踪最终命令，<strong>一律放行</strong>（宁可不拦，不可错拦）。
        </div>
      {/if}

      <!-- P127 三期：滚动基线。拦截层拦不住 `sed -i` 这类改配置的命令，靠它兜底 -->
      <label class="snap-item snap-toggle">
        <input type="checkbox" checked={baselineOn} onchange={toggleBaseline} />
        <span>
          滚动基线（改配置前自动留底）
          {#if baselineEst}<span class="ok">· 已建立</span>{/if}
        </span>
      </label>
      {#if baselineOn}
        <div class="snap-hint-sm">
          <strong>首次写操作立即拍</strong>，之后每 10 条写操作或 15 分钟更新一次。
          只读命令（<code>ls</code>/<code>grep</code>/<code>tail</code>…）<strong>不触发</strong>，故查询再多也不会有开销。
          退路 = 最近一次基线（最多丢 15 分钟）。
        </div>
      {/if}

      <div class="snap-pick">
        <div class="snap-pick-title">打包范围</div>
        {#each probe.pathSizes as [p, mb] (p)}
          <label class="snap-item">
            <input type="checkbox" checked={picked.has(p)} onchange={() => toggle(p)} />
            <code>{p}</code>
            <span class="snap-size">{mb}MB</span>
          </label>
        {/each}
      </div>

      <div class="snap-est">
        预估 <strong>{estMb}MB</strong>
        {#if estMb * 3 > probe.diskAvailMb}
          <span class="warn">· 空间偏紧（需 {estMb * 3}MB 安全余量）</span>
        {/if}
      </div>

      <button class="snap-btn primary" onclick={doPack} disabled={packing || estMb === 0}>
        {packing ? "打包中…" : "创建快照"}
      </button>
    {/if}

    {#if error}<div class="snap-error">{error}</div>{/if}
    {#if notice}<div class="snap-notice">{notice}</div>{/if}

    <div class="snap-list">
      <div class="snap-pick-title">已有快照（{entries.length}）</div>
      {#if entries.length === 0}
        <p class="snap-hint">暂无快照</p>
      {:else}
        {#each entries as e (e.file)}
          <div class="snap-row">
            <span class="snap-file" title={e.path}>{e.file}</span>
            <span class="snap-size">{fmtSize(e.size)}</span>
            <span class="snap-time">{fmtTime(e.createdAt)}</span>
            <button class="snap-del" title="删除此快照" onclick={() => doRemove(e.file)}>×</button>
          </div>
        {/each}
      {/if}
    </div>
  </div>
</ModalShell>

<style>
  .snap-head {
    display: flex;
    align-items: center;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .snap-head h3 {
    margin: 0;
    font-size: 0.9rem;
  }
  .snap-close {
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
  .snap-close:hover {
    color: var(--fg);
    background: var(--hover);
  }
  .snap-body {
    padding: 0.8rem 1rem;
    overflow: auto;
    min-height: 200px;
    max-height: 60vh;
  }
  .snap-conclusion {
    padding: 0.5rem 0.7rem;
    background: var(--input-bg);
    border-radius: var(--radius-sm);
    font-size: 0.78rem;
    line-height: 1.5;
  }
  .snap-caps {
    display: flex;
    flex-wrap: wrap;
    gap: 0.6rem;
    margin: 0.5rem 0;
    font-size: 0.72rem;
    color: var(--fg-muted);
  }
  .snap-caps .ok {
    color: var(--ok);
  }
  .snap-caps .dim {
    opacity: 0.7;
  }
  .snap-pick-title {
    font-size: 0.74rem;
    color: var(--fg-muted);
    margin: 0.6rem 0 0.3rem;
  }
  .snap-item {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.25rem 0;
    font-size: 0.78rem;
    cursor: pointer;
  }
  .snap-item code {
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
  }
  .snap-toggle {
    margin: 0.4rem 0;
    padding: 0.35rem 0.5rem;
    background: var(--input-bg);
    border-radius: var(--radius-sm);
    align-items: flex-start;
  }
  .snap-toggle input {
    margin-top: 0.15rem;
  }
  .snap-toggle .ok {
    color: var(--ok);
  }
  .snap-hint-sm {
    margin: 0.3rem 0 0.5rem;
    font-size: 0.72rem;
    line-height: 1.6;
    color: var(--fg-muted);
  }
  .snap-size {
    margin-left: auto;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
    font-size: 0.72rem;
  }
  .snap-est {
    margin: 0.5rem 0;
    font-size: 0.76rem;
  }
  .snap-est .warn {
    color: var(--warning);
  }
  .snap-btn {
    padding: 0.35rem 0.9rem;
    font-size: 0.78rem;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    cursor: pointer;
  }
  .snap-btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
    font-weight: 600;
  }
  .snap-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .snap-error {
    margin: 0.5rem 0;
    padding: 0.4rem 0.6rem;
    background: var(--warn-bg);
    border-radius: var(--radius-sm);
    font-size: 0.75rem;
    color: var(--danger);
  }
  .snap-notice {
    margin: 0.5rem 0;
    font-size: 0.75rem;
    color: var(--ok);
  }
  .snap-list {
    margin-top: 0.6rem;
    border-top: 1px solid var(--border);
    padding-top: 0.4rem;
  }
  .snap-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.3rem 0;
    font-size: 0.76rem;
    border-bottom: 1px solid var(--border);
  }
  .snap-file {
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .snap-time {
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .snap-del {
    margin-left: auto;
    border: none;
    background: transparent;
    color: var(--fg-muted);
    cursor: pointer;
    padding: 0 0.3rem;
    border-radius: var(--radius-sm);
  }
  .snap-del:hover {
    color: var(--danger);
    background: var(--hover);
  }
  .snap-hint {
    margin: 0.5rem 0;
    font-size: 0.78rem;
    color: var(--fg-muted);
  }
</style>
