<script lang="ts">
  import { statusMessage, activeTab } from '../stores/appStore';
  import { exportResultToCSV } from '../lib/csv';

  async function onExport() {
    try {
      await exportResultToCSV($activeTab?.result, 'query_results.csv');
    } catch (e: any) {
      statusMessage.set('Export failed: ' + String(e));
    }
  }
</script>

<footer class="status-bar">
  <span class="status-msg">{$statusMessage}</span>
  {#if $activeTab?.result}
    <span class="status-sep">|</span>
    <span class="status-stat">{$activeTab.result.rows?.length ?? 0} rows</span>
    <span class="status-sep">|</span>
    <span class="status-stat">{$activeTab.result.duration}ms</span>
    <span class="status-sep">|</span>
    <button class="export-btn" on:click={onExport} title="Export results to CSV">
      Export data
    </button>
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
  .export-btn {
    background: var(--accent); color: white;
    border: none; border-radius: 3px; padding: 2px 8px;
    font-size: 11px; cursor: pointer;
    margin-left: 4px;
  }
  .export-btn:hover { background: var(--accent-hover, #0056b3); }
</style>
