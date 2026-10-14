<script lang="ts">
  // P101-1 弹窗外壳：统一 dialog 语义 + Esc 关闭 + 遮罩点击关闭 + 挂载即聚焦。
  //
  // 只负责「遮罩 + 对话框容器 + 键盘/焦点行为」；标题栏/正文/页脚由调用方
  // 以 snippet 传入并保留各自的 scoped 样式（snippet 在调用方作用域编译，
  // 故调用方那些 .modal-head/.modal-body 规则照旧生效）。
  //
  // 注：SettingsModal 自带一套等价实现（P105 完成并实测），未迁移到此以免
  // 重复验证；本组件的 veil/dialog 样式与之一致，改动需两侧同步。
  import { onMount } from "svelte";
  import type { Snippet } from "svelte";

  interface Props {
    /** 标题元素 id（用于 aria-labelledby） */
    labelledBy: string;
    onClose: () => void;
    /** 各弹窗尺寸不同（会话表单 480px / 历史与预览 720px） */
    width?: string;
    /**
     * 初始高度。不传 = 高度自适应内容（旧行为）。
     * 内容里有 `flex:1` 的子元素时**必须给初始高度** —— 否则容器高度由内容决定、
     * 而内容又要撑满容器，会塌成最小高度（P130 实测踩到：弹窗只剩 160px）。
     */
    height?: string;
    /**
     * P130：允许**拖动弹窗边框**调整大小（默认关，避免影响既有弹窗布局）。
     * 可拖右边 / 下边 / 右下角；拖的是"窗口"而非内容，故内容随之重排。
     */
    resizable?: boolean;
    minWidth?: number;
    minHeight?: number;
    /**
     * 供子内容调用的缩放接口（`bind:api`）。用于给拖拽提供**键盘等价操作**——
     * 纯拖拽手柄对键盘用户不可达。
     */
    api?: { nudge: (dw: number, dh: number) => void } | null;
    children: Snippet;
  }
  let {
    labelledBy,
    onClose,
    width = "520px",
    height = undefined,
    resizable = false,
    minWidth = 380,
    minHeight = 200,
    api = $bindable(null),
    children,
  }: Props = $props();

  let dialogEl = $state<HTMLElement | null>(null);

  // P130：拖拽进行中标志。**必须**用它挡住遮罩的 onclick——
  // 否则鼠标在弹窗边缘按下、拖到遮罩上松开时，click 冒泡到 veil 会把弹窗关掉
  // （实测：拖完边框弹窗直接消失）。
  let resizing = $state(false);

  // 拖拽后的显式尺寸；null = 用默认（宽度取 width prop / 高度自适应内容）
  let dragW = $state<number | null>(null);
  let dragH = $state<number | null>(null);

  // 默认宽度（拖拽前的基准）。**不缓存成模块级常量** ——
  // 那会触发 state_referenced_locally 提示（只捕获 prop 初值），
  // 且父组件运行时改 width 时基准不会更新。改为用时现读。
  const defaultW = () => Number.parseFloat(width) || 520;
  // 边距 24px：与 .dialog.resizable 的 max-* 保持一致，
  // 否则拖拽上限与 CSS 上限不一致（会"拖到一半就不动"）。
  const clampW = (v: number) => Math.max(minWidth, Math.min(window.innerWidth - 24, v));
  const clampH = (v: number) => Math.max(minHeight, Math.min(window.innerHeight - 24, v));

  // 暴露给子内容的缩放接口（键盘/按钮路径）
  $effect(() => {
    api = {
      nudge: (dw, dh) => {
        const w = dragW ?? dialogEl?.offsetWidth ?? defaultW();
        const h = dragH ?? dialogEl?.offsetHeight ?? minHeight;
        if (dw) dragW = clampW(w + dw);
        if (dh) dragH = clampH(h + dh);
      },
    };
    return () => {
      api = null;
    };
  });

  type Edge = "r" | "b" | "br";

  function startResize(e: MouseEvent, edge: Edge) {
    e.preventDefault();
    e.stopPropagation();
    resizing = true;
    const sx = e.clientX;
    const sy = e.clientY;
    const sw = dialogEl?.offsetWidth ?? defaultW();
    const sh = dialogEl?.offsetHeight ?? minHeight;
    const onMove = (ev: MouseEvent) => {
      if (edge === "r" || edge === "br") dragW = clampW(sw + (ev.clientX - sx));
      if (edge === "b" || edge === "br") dragH = clampH(sh + (ev.clientY - sy));
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      // 延后清除：让紧随 mouseup 的 click 也被挡住（同一轮事件循环内）
      setTimeout(() => (resizing = false), 0);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

  // 焦点陷阱（P124）：记录打开前的焦点元素，关闭时归还，
  // 否则键盘用户关掉弹窗后焦点会掉到 <body>，后续 Tab 从页首重新开始。
  let previouslyFocused: HTMLElement | null = null;

  const FOCUSABLE =
    'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

  function focusables(): HTMLElement[] {
    if (!dialogEl) return [];
    return Array.from(dialogEl.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
      // 排除不可见元素（display:none / visibility:hidden 的按钮不该被 Tab 到）
      (el) => el.offsetParent !== null || el === document.activeElement,
    );
  }

  onMount(() => {
    previouslyFocused = document.activeElement as HTMLElement | null;
    // 焦点进入弹窗：Esc 与 Tab 起点都在弹窗内，且读屏能播报对话框标题
    dialogEl?.focus();
    return () => {
      // 归还焦点（元素可能已随弹窗一起卸载，故判 isConnected）
      if (previouslyFocused?.isConnected) previouslyFocused.focus();
    };
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
      return;
    }
    // 焦点陷阱：Tab 在弹窗内循环，不逃逸到背后的页面（aria-modal 的语义要求）
    if (e.key === "Tab") {
      const items = focusables();
      if (items.length === 0) {
        e.preventDefault();
        dialogEl?.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement as HTMLElement | null;
      // 焦点在弹窗容器本身（刚打开）或已在最后一个元素上 → 回到首个
      if (!active || active === dialogEl || !dialogEl?.contains(active)) {
        e.preventDefault();
        (e.shiftKey ? last : first).focus();
        return;
      }
      if (e.shiftKey && active === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<!-- 遮罩：点击空白关闭；dialog 容器 stopPropagation 防误关 -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<!-- 拖拽中不响应遮罩点击(否则拖完边框会误关弹窗) -->
<div class="veil" onclick={() => { if (!resizing) onClose(); }}>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="dialog"
    class:resizable
    role="dialog"
    aria-modal="true"
    aria-labelledby={labelledBy}
    tabindex="-1"
    bind:this={dialogEl}
    style:width={dragW !== null ? `${dragW}px` : width}
    style:height={dragH !== null ? `${dragH}px` : height}
    style:max-width={resizable ? "calc(100vw - 24px)" : null}
    style:max-height={resizable ? "calc(100vh - 24px)" : null}
    onclick={(e) => e.stopPropagation()}
  >
    {@render children()}

    {#if resizable}
      <!-- 边框拖拽手柄：拖的是弹窗本身（内容随之重排）。
           三条边各自独立，命中区比边框略宽，便于抓取。 -->
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_no_static_element_interactions -->
      <div class="rz rz-r" role="separator" aria-label="拖动调整宽度" onmousedown={(e) => startResize(e, "r")}></div>
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_no_static_element_interactions -->
      <div class="rz rz-b" role="separator" aria-label="拖动调整高度" onmousedown={(e) => startResize(e, "b")}></div>
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_no_static_element_interactions -->
      <div class="rz rz-br" role="separator" aria-label="拖动调整大小" onmousedown={(e) => startResize(e, "br")}></div>
    {/if}
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
  .dialog {
    background: var(--modal-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    /* 窄窗口下不许溢出（width 由调用方给，这里只兜底） */
    max-width: calc(100vw - 32px);
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    outline: none;
    position: relative;
  }
  /* P130 修正：可拖拽时**不能**把上限设成 none ——
     初始 height 是固定像素(如 520px)，小窗口下会超出视口把弹窗裁掉
     （实测 515px 视口 + 520px 弹窗 → 顶部 -2px、底部超出 3px）。
     故上限始终跟随视口，拖拽时由 clampW/clampH 再做一次精确钳制。 */
  .dialog.resizable {
    max-width: calc(100vw - 24px);
    max-height: calc(100vh - 24px);
  }

  /* ---------- P130 边框拖拽手柄 ---------- */
  .rz {
    position: absolute;
    z-index: 2;
  }
  .rz-r {
    top: 0;
    right: 0;
    width: 6px;
    height: 100%;
    cursor: ew-resize;
  }
  .rz-b {
    left: 0;
    bottom: 0;
    width: 100%;
    height: 6px;
    cursor: ns-resize;
  }
  .rz-br {
    right: 0;
    bottom: 0;
    width: 16px;
    height: 16px;
    cursor: nwse-resize;
  }
  /* 悬停时给边框一点视觉提示（手柄本身透明，不遮挡内容） */
  .rz-r:hover,
  .rz-b:hover,
  .rz-br:hover {
    background: var(--accent);
    opacity: 0.5;
  }
  .rz-br::after {
    content: "";
    position: absolute;
    right: 3px;
    bottom: 3px;
    width: 7px;
    height: 7px;
    border-right: 2px solid var(--fg-muted);
    border-bottom: 2px solid var(--fg-muted);
    opacity: 0.7;
  }
</style>
