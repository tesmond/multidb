<script lang="ts">
  import { outputTab, activeTab, queryHistoryStore, tabs, activeTabId, selectedConnId } from '../stores/appStore';
  import ResultsGrid from './ResultsGrid.svelte';
  import { get } from 'svelte/store';

  const outputTabs: Array<['results' | 'messages' | 'history', string]> = [
    ['results', 'Results'],
    ['messages', 'Messages'],
    ['history', 'History'],
  ];

  function useHistoryQuery(query: string) {
    const connId = get(selectedConnId);
    tabs.add(connId);
    const allTabs = get(tabs);
    const newTab = allTabs[allTabs.length - 1];
    tabs.updateTab(newTab.id, { sql: query });
    activeTabId.set(newTab.id);
  }
</script>

<div class="output-panel">
  <div class="output-tabs" role="tablist">
    {#each outputTabs as [key, label]}
      <button
        class="output-tab"
        class:active={$outputTab === key}
        on:click={() => outputTab.set(key)}
        role="tab"
        aria-selected={$outputTab === key}
      >{label}</button>
    {/each}
  </div>

  <div class="output-content">
    {#if $outputTab === 'results'}
      <ResultsGrid result={$activeTab?.result ?? null} />

    {:else if $outputTab === 'messages'}
      <div class="messages">
        {#if $activeTab?.result}
          {#if $activeTab.result.error}
            <div class="msg error">ERROR: {$activeTab.result.error}</div>
          {:else}
            <div class="msg success">
              Query OK — {$activeTab.result.rowsAffected} row(s) affected, {$activeTab.result.rows?.length ?? 0} row(s) returned in {$activeTab.result.duration}ms
            </div>
          {/if}
        {:else}
          <div class="msg muted">No messages.</div>
        {/if}
      </div>

    {:else if $outputTab === 'history'}
      <div class="history-list">
        {#each $queryHistoryStore as rec (rec.id)}
          <div class="history-item">
            <div class="history-query" on:click={() => useHistoryQuery(rec.query)} role="button" tabindex="0" on:keydown={e => e.key === 'Enter' && useHistoryQuery(rec.query)}>
              <code>{rec.query.length > 120 ? rec.query.slice(0, 120) + '…' : rec.query}</code>
            </div>
            <div class="history-meta">
              <span>{rec.createdAt}</span>
              <span>{rec.duration}ms</span>
              {#if rec.error}<span class="err-badge">ERR</span>{/if}
            </div>
          </div>
        {/each}
        {#if $queryHistoryStore.length === 0}
          <div class="msg muted">No query history yet.</div>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .output-panel { display: flex; flex-direction: column; height: 100%; overflow: hidden; }
  .output-tabs {
    display: flex; gap: 0;
    border-bottom: 1px solid var(--border);
    background: var(--bg-panel);
    flex-shrink: 0;
  }
  .output-tab {
    padding: 6px 16px; background: none; border: none;
    border-bottom: 2px solid transparent;
    color: var(--text-muted); font-size: 12px; cursor: pointer;
  }
  .output-tab:hover { color: var(--text); }
  .output-tab.active { color: var(--text); border-bottom-color: var(--accent); }
  .output-content { flex: 1; overflow: hidden; }

  .messages, .history-list { height: 100%; overflow-y: auto; padding: 8px 12px; }
  .msg { font-size: 12px; padding: 6px 10px; border-radius: 4px; }
  .msg.error { color: var(--error); background: rgba(255,80,80,0.08); }
  .msg.success { color: var(--success); }
  .msg.muted { color: var(--text-muted); }

  .history-item {
    padding: 6px 8px; border-bottom: 1px solid var(--border-subtle);
    cursor: default;
  }
  .history-query {
    cursor: pointer; color: var(--text);
  }
  .history-query:hover code { color: var(--accent); }
  .history-query code {
    font-family: 'JetBrains Mono', 'Fira Code', monospace;
    font-size: 11px; white-space: pre-wrap; word-break: break-all;
  }
  .history-meta {
    display: flex; gap: 12px; font-size: 10px; color: var(--text-muted);
    margin-top: 2px;
  }
  .err-badge { color: var(--error); font-weight: 600; }
</style>
