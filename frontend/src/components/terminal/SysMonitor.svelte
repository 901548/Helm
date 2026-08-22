<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../lib/api";

  interface Props {
    activeTab: string | null;
  }

  let { activeTab } = $props<Props>();

  let sys = $state<{
    cpu: number;
    memUsedMb: number;
    memTotalMb: number;
    load1: number;
    load5: number;
    load15: number;
    txBps: number;
    rxBps: number;
    ok: boolean;
    error?: string;
  } | null>(null);

  function fmtBytes(bps: number): string {
    if (bps >= 1024 * 1024) return (bps / 1024 / 1024).toFixed(1) + "M/s";
    if (bps >= 1024) return (bps / 1024).toFixed(0) + "K/s";
    return bps + "B/s";
  }

  function fmtMb(mb: number): string {
    if (mb >= 1024) return (mb / 1024).toFixed(1) + "G";
    return mb + "M";
  }

  $effect(() => {
    activeTab;
    sys = null;
  });

  onMount(() => {
    // onSysMon 返回 Promise<UnlistenFn>,须暂存后异步注销,直接 return Promise 会被忽略泄漏
    let un: (() => void) | null = null;
    api.onSysMon((p) => {
      if (p.name !== activeTab) return;
      sys = {
        cpu: p.cpu,
        memUsedMb: p.memUsedMb,
        memTotalMb: p.memTotalMb,
        load1: p.load1,
        load5: p.load5,
        load15: p.load15,
        txBps: p.txBps,
        rxBps: p.rxBps,
        ok: p.ok,
        error: p.error,
      };
    }).then((fn) => (un = fn));
    return () => un?.();
  });
</script>

<div class="sysmon">
  {#if sys}
    {#if sys.ok}
      <span class="item" title="CPU 使用率">
        <span class="bar-wrap" class:hot={sys.cpu > 80}><span class="bar" class:hot={sys.cpu > 80} style:width={`${Math.min(100, sys.cpu)}%`}></span></span>
        <span class="lbl">CPU</span> <b>{Math.round(sys.cpu)}%</b>
      </span>
      <span class="sep" aria-hidden="true"></span>
      <span class="item" title="内存">
        <span class="bar-wrap" class:hot={sys.memTotalMb > 0 && (sys.memUsedMb / sys.memTotalMb) * 100 > 85}>
          <span class="bar mem" class:hot={sys.memTotalMb > 0 && (sys.memUsedMb / sys.memTotalMb) * 100 > 85} style:width={`${sys.memTotalMb ? Math.min(100, (sys.memUsedMb / sys.memTotalMb) * 100) : 0}%`}></span>
        </span>
        <span class="lbl">MEM</span> <b>{fmtMb(sys.memUsedMb)}/{fmtMb(sys.memTotalMb)}</b>
      </span>
      <span class="sep" aria-hidden="true"></span>
      <span class="item" title="负载 1/5/15 分钟">
        <span class="lbl">负载</span> <b>{sys.load1.toFixed(2)}</b> <span class="dim">/ {sys.load5.toFixed(2)} / {sys.load15.toFixed(2)}</span>
      </span>
      <span class="sep" aria-hidden="true"></span>
      <span class="item mono" title="网络速率">
        <span class="net up">↑</span><b>{fmtBytes(sys.txBps)}</b>
        <span class="net down">↓</span><b>{fmtBytes(sys.rxBps)}</b>
      </span>
    {:else}
      <span class="item err">— {sys.error ?? "无法获取系统状态"}</span>
    {/if}
  {:else}
    <span class="item dim">系统状态待连接…</span>
  {/if}
</div>

<style>
  .sysmon {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 0.2rem 0.7rem;
    font-size: 0.72rem;
    color: var(--term-fg);
    background: var(--term-bg);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
    user-select: none;
    white-space: nowrap;
    overflow: hidden;
  }
  .item {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
  }
  .item b {
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .lbl {
    color: var(--fg-muted);
    font-size: 0.68rem;
    letter-spacing: 0.05em;
  }
  .dim {
    color: var(--fg-muted);
  }
  .err {
    color: var(--danger);
  }
  .mono {
    font-family: "Cascadia Mono", monospace;
  }
  .sep {
    width: 1px;
    height: 11px;
    background: var(--border);
    flex-shrink: 0;
  }
  .net {
    font-size: 0.68rem;
  }
  .net.up {
    color: var(--ok);
  }
  .net.down {
    color: var(--accent);
  }
  .bar-wrap {
    width: 46px;
    height: 5px;
    border-radius: 3px;
    background: var(--track-bg);
    overflow: hidden;
    flex-shrink: 0;
  }
  .bar {
    display: block;
    height: 100%;
    background: var(--ok);
    border-radius: 3px;
    transition: width 0.4s ease;
  }
  .bar.mem {
    background: var(--accent);
  }
  /* 过热(CPU>80% / MEM>85%)转警示色 */
  .bar.hot,
  .bar.mem.hot {
    background: var(--warning);
  }
</style>
