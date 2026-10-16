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
  // P127 四期：还原对话框
  import RestoreModal from "./RestoreModal.svelte";

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
  /** P127 四期：正在还原的快照 */
  let restoreTarget = $state<SnapshotEntry | null>(null);

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
      // 首次加载用探测推荐值预勾选。
      // **与 pickable 取交集**：picked 若含列表里看不到的路径，它会被算进
      // `estMb` 却无法取消（估值与所见不符）。后端目前已按 MAX_SIZE_MB 过滤，
      // 这里再取一次交集，保证「所见即所算」，后端改常量也不会静默错位。
      if (picked.size === 0) {
        picked = new Set(
          p.recommended.filter((x) =>
            p.pathSizes.some(([q, mb]) => q === x && mb <= MAX_MB),
          ),
        );
      }
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

  /// MB → 人类可读。>= 1024MB 用 GB，否则全是四五位数字（12293MB / 2054MB）看着累
  function fmtMb(mb: number): string {
    return mb >= 1024 ? (mb / 1024).toFixed(1) + "GB" : mb + "MB";
  }

  function fmtTime(ts: number): string {
    if (!ts) return "—";
    const d = new Date(ts * 1000);
    const p = (n: number) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
  }

  /// 与后端 `snapshot.rs` 的 `MAX_SIZE_MB` 一致。
  /// **超过它的目录不列出来**：列了也能勾，但打包必被体积检查拒绝 ——
  /// 那是"勾了白勾"的误导选项，正是面板显得乱的主因之一。
  const MAX_MB = 500;

  /// 可选的打包路径（只列真正能打包的）
  const pickable = $derived(
    (probe?.pathSizes ?? []).filter(([, mb]) => mb <= MAX_MB),
  );
  /// 被跳过的路径名（只在下面提一句"为什么没有它"，不再逐个列出来占地方）
  const skippedNames = $derived(
    (probe?.pathSizes ?? [])
      .filter(([, mb]) => mb > MAX_MB)
      .map(([p]) => p)
      .join("、"),
  );
</script>

<ModalShell
  labelledBy="snap-title"
  width="640px"
  height="510px"
  resizable
  minWidth={480}
  minHeight={360}
  {onClose}
>
  <header class="snap-head">
    <h3 id="snap-title">服务器快照</h3>
    <button class="snap-close" title="关闭 (Esc)" onclick={onClose}>×</button>
  </header>

  <div class="snap-body">
    {#if loading}
      <p class="snap-hint">探测中…</p>
    {:else if probe}
      <!-- 区块 1：自动保护（两个开关归为一组，视觉上独立于打包设置） -->
      <section class="snap-sec">
        <div class="snap-sec-title">自动保护</div>

        <!-- P127 二期：命令前拦截。默认关 —— 会拦输入，须显式开启。
             正文只留一句"它会替你做什么"；边界细节（只拦 Critical / 历史与补全放行）
             放进 title，需要的人 hover 得到，不需要的人不被干扰。 -->
        <label
          class="snap-toggle"
          title="只拦 Critical 级（系统级不可逆）；rm -rf /etc/xxx 等不拦。↑ 历史回放与 Tab 补全无法追踪最终命令，一律放行（宁可不拦，不可错拦）。"
        >
          <input type="checkbox" checked={interceptOn} onchange={toggleIntercept} />
          <div class="snap-toggle-txt">
            <div class="snap-toggle-name">
              危险命令前拦截
              {#if interceptOn}<span class="pill on">已开启</span>{:else}<span class="pill">关闭</span>{/if}
            </div>
            <div class="snap-toggle-desc">执行 <code>rm -rf /</code> 这类命令前拦一下</div>
          </div>
        </label>

        <!-- P127 三期：滚动基线。拦截层拦不住 `sed -i` 这类改配置的命令，靠它兜底 -->
        <label
          class="snap-toggle"
          title="首次写操作立即拍，之后每 10 条写操作或 15 分钟更新一次。只读命令（ls/grep/tail…）不触发，故查询再多也不会有开销。退路 = 最近一次基线（最多丢 15 分钟）。"
        >
          <input type="checkbox" checked={baselineOn} onchange={toggleBaseline} />
          <div class="snap-toggle-txt">
            <div class="snap-toggle-name">
              滚动基线
              {#if baselineOn}<span class="pill on">已开启</span>{:else}<span class="pill">关闭</span>{/if}
              {#if baselineEst}<span class="pill ok">已建立</span>{/if}
            </div>
            <div class="snap-toggle-desc">改配置前自动留底，查询类命令不打扰</div>
          </div>
        </label>
      </section>

      <!-- 区块 2：打包范围。**只列真能打包的目录**（超上限的不列，勾了也白勾）。 -->
      <section class="snap-sec">
        <div class="snap-sec-title">
          打包范围
          <span class="snap-sec-note">{pickable.length} 个目录</span>
        </div>
        <div class="snap-paths">
          {#each pickable as [p, mb] (p)}
            <label class="snap-path">
              <input type="checkbox" checked={picked.has(p)} onchange={() => toggle(p)} />
              <code>{p}</code>
              <span class="snap-size">{fmtMb(mb)}</span>
            </label>
          {/each}
        </div>
        {#if skippedNames}
          <div class="snap-skip" title="单目录超过 {MAX_MB}MB 会被体积检查拒绝，故不列出">
            已跳过 {skippedNames}（超过 {MAX_MB}MB）
          </div>
        {/if}
        {#if estMb * 3 > probe.diskAvailMb}
          <div class="snap-warn">空间偏紧：需 {fmtMb(estMb * 3)} 安全余量，当前可用 {fmtMb(probe.diskAvailMb)}</div>
        {/if}
      </section>

      <!-- 区块 3：已有快照 -->
      <section class="snap-sec">
        <div class="snap-sec-title">
          已有快照
          <span class="snap-sec-note">{entries.length} 份</span>
        </div>
        {#if entries.length === 0}
          <p class="snap-hint">暂无快照</p>
        {:else}
          {#each entries as e (e.file)}
            <div class="snap-row">
              <div class="snap-row-main">
                <span class="snap-file" title={e.path}>{e.file}</span>
                <span class="snap-meta">{fmtSize(e.size)} · {fmtTime(e.createdAt)}</span>
              </div>
              <button class="snap-restore" title="还原此快照（会先预演）" onclick={() => (restoreTarget = e)}>还原</button>
              <button class="snap-del" title="删除此快照" onclick={() => doRemove(e.file)}>×</button>
            </div>
          {/each}
        {/if}
      </section>
    {/if}

    {#if error}<div class="snap-error">{error}</div>{/if}
    {#if notice}<div class="snap-notice">{notice}</div>{/if}
  </div>

  <!-- 主操作固定在底栏。磁盘可用放这里（次要信息，不占正文）。 -->
  <footer class="snap-foot">
    <span class="snap-foot-note">
      {#if estMb > 0}约 {fmtMb(estMb)}{/if}
      {#if probe}<span class="snap-foot-disk">磁盘可用 {fmtMb(probe.diskAvailMb)}</span>{/if}
    </span>
    <button class="snap-btn" onclick={onClose}>关闭</button>
    <button class="snap-btn primary" onclick={doPack} disabled={packing || estMb === 0}>
      {packing ? "打包中…" : "创建快照"}
    </button>
  </footer>
</ModalShell>

{#if restoreTarget}
  <RestoreModal
    session={session}
    pack={restoreTarget.path}
    file={restoreTarget.file}
    onClose={() => (restoreTarget = null)}
    onRestored={load}
  />
{/if}

<style>
  /* 三段式：固定头（标题/摘要）+ 滚动主体 + 固定底栏。
     `.snap-body` 必须 flex:1 + min-height:0，否则内容会把弹窗撑破（列表被裁）。 */
  .snap-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.55rem 0.9rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .snap-head h3 {
    margin: 0;
    font-size: 0.88rem;
    font-weight: 600;
  }
  .snap-close {
    margin-left: auto;
    border: none;
    background: transparent;
    color: var(--fg-muted);
    font-size: 1.1rem;
    line-height: 1;
    padding: 0.1rem 0.35rem;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .snap-close:hover {
    color: var(--fg);
    background: var(--hover);
  }

  /* 滚动主体 */
  .snap-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    /* 底部留白：让最后一条能完全滚出底栏边界，不被"切一半" */
    padding: 0 0.9rem 0.9rem;
  }

  /* 分区 */
  .snap-sec {
    padding: 0.65rem 0 0.3rem;
  }
  .snap-sec + .snap-sec {
    border-top: 1px solid var(--border);
  }
  .snap-sec-title {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    font-size: 0.73rem;
    font-weight: 600;
    color: var(--fg-muted);
    letter-spacing: 0.03em;
    margin-bottom: 0.35rem;
  }
  .snap-sec-note {
    margin-left: auto;
    font-weight: 400;
    font-variant-numeric: tabular-nums;
  }

  /* 开关卡片 */
  .snap-toggle {
    display: flex;
    align-items: flex-start;
    gap: 0.55rem;
    padding: 0.45rem 0.6rem;
    margin-bottom: 0.35rem;
    background: var(--input-bg);
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .snap-toggle:hover {
    border-color: var(--border);
  }
  .snap-toggle input {
    margin-top: 0.15rem;
    flex-shrink: 0;
  }
  .snap-toggle-txt {
    min-width: 0;
  }
  .snap-toggle-name {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.79rem;
    font-weight: 500;
  }
  .snap-toggle-desc {
    margin-top: 0.15rem;
    font-size: 0.72rem;
    line-height: 1.55;
    color: var(--fg-muted);
  }
  .pill {
    font-size: 0.64rem;
    font-weight: 400;
    padding: 0.02rem 0.35rem;
    border-radius: 999px;
    background: var(--track-bg);
    color: var(--fg-muted);
  }
  .pill.on {
    background: var(--accent);
    color: #fff;
  }
  .pill.ok {
    background: transparent;
    border: 1px solid var(--ok);
    color: var(--ok);
  }

  /* 路径列表 */
  .snap-paths {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
  }
  .snap-path {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.22rem 0.4rem;
    border-radius: var(--radius-sm);
    font-size: 0.77rem;
    cursor: pointer;
  }
  .snap-path:hover {
    background: var(--hover);
  }
  .snap-path code {
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.77rem;
  }
  .snap-size {
    margin-left: auto;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
    font-size: 0.72rem;
    flex-shrink: 0;
  }
  /* 跳过项只提一句"为什么没有它"，不再逐个列出来占地方 */
  .snap-skip {
    margin-top: 0.25rem;
    font-size: 0.7rem;
    color: var(--fg-muted);
    opacity: 0.75;
    cursor: help;
  }
  .snap-warn {
    margin-top: 0.3rem;
    font-size: 0.72rem;
    color: var(--warning);
  }

  /* 快照列表 */
  .snap-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.35rem 0.4rem;
    border-radius: var(--radius-sm);
  }
  .snap-row:hover {
    background: var(--hover);
  }
  .snap-row-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.05rem;
  }
  .snap-file {
    font-family: "JetBrains Mono", "Cascadia Mono", monospace;
    font-size: 0.75rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .snap-meta {
    font-size: 0.7rem;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .snap-restore {
    flex-shrink: 0;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    cursor: pointer;
    padding: 0.1rem 0.45rem;
    border-radius: var(--radius-sm);
    font-size: 0.7rem;
  }
  .snap-restore:hover {
    color: var(--accent);
    border-color: var(--accent);
  }
  .snap-del {
    flex-shrink: 0;
    border: none;
    background: transparent;
    color: var(--fg-muted);
    cursor: pointer;
    padding: 0 0.3rem;
    border-radius: var(--radius-sm);
    font-size: 0.85rem;
  }
  .snap-del:hover {
    color: var(--danger);
    background: var(--hover);
  }

  /* 固定底栏 */
  .snap-foot {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.55rem 0.9rem;
    border-top: 1px solid var(--border);
    flex-shrink: 0;
  }
  .snap-foot-note {
    margin-right: auto;
    display: flex;
    align-items: center;
    gap: 0.6rem;
    font-size: 0.72rem;
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
  }
  .snap-foot-disk {
    opacity: 0.7;
  }
  .snap-btn {
    padding: 0.32rem 0.85rem;
    font-size: 0.77rem;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    cursor: pointer;
  }
  .snap-btn:hover:not(:disabled) {
    background: var(--hover);
  }
  .snap-btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
    font-weight: 600;
  }
  .snap-btn.primary:hover:not(:disabled) {
    filter: brightness(1.1);
  }
  .snap-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .snap-error {
    margin: 0.4rem 0;
    padding: 0.4rem 0.6rem;
    background: var(--warn-bg);
    border-radius: var(--radius-sm);
    font-size: 0.74rem;
    color: var(--danger);
  }
  .snap-notice {
    margin: 0.4rem 0;
    font-size: 0.74rem;
    color: var(--ok);
  }
  .snap-hint {
    margin: 0.4rem 0;
    font-size: 0.76rem;
    color: var(--fg-muted);
  }
</style>
