<script lang="ts">
  import { statusMessage, activeTab } from '../stores/appStore';
</script>

<footer class="status-bar">
  <span class="status-msg">{$statusMessage}</span>
  {#if $activeTab?.result}
    <span class="status-sep">|</span>
    <span class="status-stat">{$activeTab.result.rows?.length ?? 0} rows</span>
    <span class="status-sep">|</span>
    <span class="status-stat">{$activeTab.result.duration}ms</span>
  {/if}
</footer>

<style>
  .status-bar {
    display: flex; align-items: center; gap: 6px;
    height: 24px; padding: 0 12px;
    background: var(--bg-toolbar);
    border-top: 1px solid var(--border);
    font-size: 11px; color: var(--text-muted);
    flex-shrink: 0;
  }
  .status-msg { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .status-sep { opacity: 0.4; }
  .status-stat { color: var(--text-dim); }
</style>
