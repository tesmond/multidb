<script lang="ts">
  import { statusMessage, activeTab } from '../stores/appStore';

  function escapeCSV(value: any): string {
    if (value === null) return '';
    const str = String(value);
    // If the value contains comma, quote, or newline, wrap in quotes and escape quotes
    if (str.includes(',') || str.includes('"') || str.includes('\n') || str.includes('\r')) {
      return '"' + str.replace(/"/g, '""') + '"';
    }
    return str;
  }

  function exportToCSV() {
    const result = $activeTab?.result;
    if (!result || !result.columns || !result.rows) return;

    const csvRows = [
      // Header row
      result.columns.map(col => escapeCSV(col)).join(','),
      // Data rows
      ...result.rows.map(row => row.map(cell => escapeCSV(cell)).join(','))
    ];

    const csvContent = csvRows.join('\n');
    const blob = new Blob([csvContent], { type: 'text/csv;charset=utf-8;' });
    const link = document.createElement('a');

    if (link.download !== undefined) {
      const url = URL.createObjectURL(blob);
      link.setAttribute('href', url);
      link.setAttribute('download', 'query_results.csv');
      link.style.visibility = 'hidden';
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
      URL.revokeObjectURL(url);
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
    <button class="export-btn" on:click={exportToCSV} title="Export results to CSV">
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
