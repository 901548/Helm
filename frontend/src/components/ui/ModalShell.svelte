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
    children: Snippet;
  }
  let { labelledBy, onClose, width = "520px", children }: Props = $props();

  let dialogEl = $state<HTMLElement | null>(null);

  // 焦点进入弹窗：Esc 与 Tab 起点都在弹窗内，且读屏能播报对话框标题
  onMount(() => dialogEl?.focus());

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<!-- 遮罩：点击空白关闭；dialog 容器 stopPropagation 防误关 -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="veil" onclick={onClose}>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby={labelledBy}
    tabindex="-1"
    bind:this={dialogEl}
    style:width
    onclick={(e) => e.stopPropagation()}
  >
    {@render children()}
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
  }
</style>
