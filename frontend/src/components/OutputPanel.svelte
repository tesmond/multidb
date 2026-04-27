<script lang="ts">
  import { onMount } from 'svelte';
  import { outputTab, activeTab, queryHistoryStore, tabs, activeTabId, selectedConnId, activeConnections } from '../stores/appStore';

  // Derive connection ID from the active tab
  $: activeTabConnId = $activeTab?.connId ?? '';
  import ResultsGrid from './ResultsGrid.svelte';
  import EditSavedQueryTitleDialog from './EditSavedQueryTitleDialog.svelte';
  import { GetQueryHistoryByConnID, SaveAndConnect, ClearQueryHistoryByConnID, ClearQueryHistory } from '../../wailsjs/go/main/App';
  import { get } from 'svelte/store';

  let contextMenu: { x: number; y: number; type: 'history' | 'saved'; itemId?: number; itemTitle?: string } | null = null;
  let editTitleDialog: EditSavedQueryTitleDialog;

  function openContextMenu(e: MouseEvent, type: 'history' | 'saved', itemId?: number, itemTitle?: string) {
    contextMenu = { x: e.clientX, y: e.clientY, type, itemId, itemTitle };
  }

  function closeContextMenu() {
    contextMenu = null;
  }

  async function clearConnectionHistory(e?: Event) {
    if (e) e.stopPropagation();
    closeContextMenu();
    const connId = $activeTab?.connId ?? get(selectedConnId);
    if (connId) {
      await ClearQueryHistoryByConnID(connId);
    } else {
      await ClearQueryHistory();
    }
    connectionHistory = [];
  }

  async function clearAllHistory(e?: Event) {
    if (e) e.stopPropagation();
    closeContextMenu();
    await ClearQueryHistory();
    connectionHistory = [];
  }

  let hasSavedQueries = false;
  
  let outputTabs: Array<['results' | 'messages' | 'history' | 'saved', string]> = [
    ['results', 'Results'],
    ['messages', 'Messages'],
    ['history', 'History'],
  ];

  let connectionHistory: import('../stores/appStore').QueryRecord[] = [];
  let connectionSavedQueries: import('../stores/appStore').SavedQuery[] = [];

  // Load history for the current connection
  async function loadConnectionHistory() {
    const connId = $activeTab?.connId ?? get(selectedConnId);
    if (connId) {
      try {
        const history = await GetQueryHistoryByConnID(connId, 50);
        connectionHistory = history || [];
      } catch (e) {
        console.error('Failed to load connection history:', e);
        connectionHistory = [];
      }
    } else {
      connectionHistory = [];
    }
  }

  // Load saved queries for the current connection
  async function loadSavedQueries() {
    const connId = $activeTab?.connId ?? get(selectedConnId);
    if (connId) {
      try {
        const app = await import('../../wailsjs/go/main/App') as any;
        const saved = await app.GetSavedQueries(connId);
        connectionSavedQueries = saved || [];
        hasSavedQueries = (saved?.length ?? 0) > 0;
        // Update the tabs list if needed
        if (hasSavedQueries && !outputTabs.find(t => t[0] === 'saved')) {
          outputTabs = [...outputTabs, ['saved', 'Saved']];
        } else if (!hasSavedQueries && outputTabs.find(t => t[0] === 'saved') && $outputTab !== 'saved') {
          outputTabs = outputTabs.filter(t => t[0] !== 'saved');
        }
      } catch (e) {
        console.error('Failed to load saved queries:', e);
        connectionSavedQueries = [];
      }
    } else {
      connectionSavedQueries = [];
    }
  }

  // Reload history when active tab's connection changes or history tab is selected
  $: if (activeTabConnId && $outputTab === 'history') loadConnectionHistory();
  $: if (activeTabConnId && $outputTab === 'saved') loadSavedQueries();
  $: if (activeTabConnId && ($outputTab === 'results' || $outputTab === 'messages')) loadSavedQueries();

  // Also load history when switching to history tab
  function handleTabClick(tab: 'results' | 'messages' | 'history' | 'saved') {
    outputTab.set(tab);
    if (tab === 'history') {
      loadConnectionHistory();
    } else if (tab === 'saved') {
      loadSavedQueries();
    }
  }

  onMount(() => {
    // Load history initially if we're on the history tab
    if (get(outputTab) === 'history') {
      loadConnectionHistory();
    } else if (get(outputTab) === 'saved') {
      loadSavedQueries();
    } else {
      loadSavedQueries();
    }

    // Listen for saved-query-added event
    const handleSavedQueryAdded = () => {
      loadSavedQueries();
    };
    window.addEventListener('saved-query-added', handleSavedQueryAdded);
    return () => {
      window.removeEventListener('saved-query-added', handleSavedQueryAdded);
    };
  });

  async function useSavedQuery(query: string, connId: string) {
    // Check if connection is already active
    const activeConns = get(activeConnections);
    const isActive = activeConns.some(conn => conn.config.id === connId);

    if (!isActive) {
      try {
        const { ListSavedConnections } = await import('../../wailsjs/go/main/App');
        const savedConns = await ListSavedConnections();
        const savedConn = savedConns.find(c => c.id === connId);

        if (savedConn) {
          await SaveAndConnect(savedConn);
          activeConnections.update(conns =>
            [...conns, { config: savedConn, schema: null, schemaLoading: false, schemaError: null }]
          );
        } else {
          console.error('Saved connection not found');
          return;
        }
      } catch (e) {
        console.error('Failed to connect to database:', e);
        return;
      }
    }

    selectedConnId.set(connId);
    tabs.add(connId);
    const allTabs = get(tabs);
    const newTab = allTabs[allTabs.length - 1];
    tabs.updateTab(newTab.id, { sql: query });
    activeTabId.set(newTab.id);
  }

  async function useHistoryQuery(query: string, connId: string) {
    // Check if connection is already active
    const activeConns = get(activeConnections);
    const isActive = activeConns.some(conn => conn.config.id === connId);

    if (!isActive) {
      // Find the saved connection config and connect to it
      try {
        // We need to get the saved connection config
        // For now, let's assume we can get it from somewhere
        // Actually, let me check if we can get saved connections
        const { ListSavedConnections } = await import('../../wailsjs/go/main/App');
        const savedConns = await ListSavedConnections();
        const savedConn = savedConns.find(c => c.id === connId);

        if (savedConn) {
          await SaveAndConnect(savedConn);
          // Add to active connections
          activeConnections.update(conns =>
            [...conns, { config: savedConn, schema: null, schemaLoading: false, schemaError: null }]
          );
        } else {
          console.error('Saved connection not found for history item');
          return;
        }
      } catch (e) {
        console.error('Failed to connect to database for history item:', e);
        return;
      }
    }

    // Switch to the correct connection
    selectedConnId.set(connId);

    // Create new tab with the query
    tabs.add(connId);
    const allTabs = get(tabs);
    const newTab = allTabs[allTabs.length - 1];
    tabs.updateTab(newTab.id, { sql: query });
    activeTabId.set(newTab.id);
  }

  async function deleteSavedQuery(e?: Event) {
    if (e) e.stopPropagation();
    if (!contextMenu || contextMenu.type !== 'saved' || !contextMenu.itemId) return;
    try {
      const app = await import('../../wailsjs/go/main/App') as any;
      await app.DeleteSavedQuery(contextMenu.itemId);
      await loadSavedQueries();
    } catch (err) {
      console.error('Failed to delete saved query:', err);
    }
    closeContextMenu();
  }

  async function editSavedQueryTitle(e?: Event) {
    if (e) e.stopPropagation();
    if (!contextMenu || contextMenu.type !== 'saved' || !contextMenu.itemId) return;
    const savedId = contextMenu.itemId;
    const currentTitle = contextMenu.itemTitle ?? '';
    closeContextMenu();

    const newTitle = await editTitleDialog.open(currentTitle);
    if (!newTitle || newTitle === currentTitle) return;

    try {
      const app = await import('../../wailsjs/go/main/App') as any;
      await app.UpdateSavedQueryTitle(savedId, newTitle);
      await loadSavedQueries();
    } catch (e) {
      console.error('Failed to update saved query title:', e);
    }
  }
</script>

<div class="output-panel">
  <div class="output-tabs" role="tablist">
    {#each outputTabs as [key, label]}
      <button
        class="output-tab"
        class:active={$outputTab === key}
        on:click={() => handleTabClick(key)}
        role="tab"
        aria-selected={$outputTab === key}
      >{label}</button>
    {/each}
  </div>

  <div class="output-content">
    {#if $outputTab === 'results'}
      <ResultsGrid result={$activeTab?.result ?? null} tabId={$activeTab?.id ?? ''} />

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
      <div class="history-list" on:contextmenu|preventDefault on:mousedown={e => e.button === 2 && openContextMenu(e, 'history')} role="list">
        {#each connectionHistory as rec (rec.id)}
          <div class="history-item">
            <div class="history-query" on:click={() => useHistoryQuery(rec.query, rec.connId)} role="button" tabindex="0" on:keydown={e => e.key === 'Enter' && useHistoryQuery(rec.query, rec.connId)}>
              <code>{rec.query.length > 120 ? rec.query.slice(0, 120) + '…' : rec.query}</code>
            </div>
            <div class="history-meta">
              <span>{rec.createdAt}</span>
              <span>{rec.duration}ms</span>
              {#if rec.resultCount !== undefined}<span>{rec.resultCount} rows</span>{/if}
              {#if rec.error}<span class="err-badge">ERR</span>{/if}
            </div>
          </div>
        {/each}
        {#if connectionHistory.length === 0}
          <div class="msg muted">No query history for this connection yet.</div>
        {/if}
      </div>

    {:else if $outputTab === 'saved'}
      <div class="saved-list" role="list">
        {#each connectionSavedQueries as saved (saved.id)}
          <div class="saved-item" on:contextmenu|preventDefault={(e) => openContextMenu(e, 'saved', saved.id, saved.title)}>
            <div
              class="saved-title"
              on:click={() => useSavedQuery(saved.query, saved.connId)}
              role="button"
              tabindex="0"
              on:keydown={(e) => e.key === 'Enter' && useSavedQuery(saved.query, saved.connId)}
            >
              {saved.title}
            </div>
            <div class="saved-meta">
              <span title={saved.query}>{saved.query.length > 100 ? saved.query.slice(0, 100) + '…' : saved.query}</span>
              <span>{new Date(saved.createdAt).toLocaleString()}</span>
            </div>
          </div>
        {/each}
        {#if connectionSavedQueries.length === 0}
          <div class="msg muted">No saved queries for this connection yet.</div>
        {/if}
      </div>
    {/if}
  </div>
</div>

<EditSavedQueryTitleDialog bind:this={editTitleDialog} />

{#if contextMenu}
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <div class="ctx-backdrop" on:click={closeContextMenu} role="presentation"></div>
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <div class="ctx-menu" style="left:{contextMenu.x}px; top:{contextMenu.y}px" on:click={(e) => e.stopPropagation()}>
    {#if contextMenu.type === 'history'}
      <button on:click={(e) => clearConnectionHistory(e)}>Clear connection history</button>
      <button on:click={(e) => clearAllHistory(e)}>Clear all history</button>
    {:else if contextMenu.type === 'saved'}
      <button on:click={(e) => editSavedQueryTitle(e)}>Edit title</button>
      <button on:click={(e) => deleteSavedQuery(e)}>Delete</button>
    {/if}
  </div>
{/if}

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

  .messages, .history-list, .saved-list { height: 100%; overflow-y: auto; padding: 8px 12px; }
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

  .saved-item {
    padding: 8px; border-bottom: 1px solid var(--border-subtle);
    cursor: default;
  }
  .saved-title {
    cursor: pointer; color: var(--text); font-weight: 500; font-size: 12px;
  }
  .saved-title:hover { color: var(--accent); }
  .saved-meta {
    display: flex; flex-direction: column; gap: 2px; font-size: 10px; color: var(--text-muted);
    margin-top: 4px;
  }
  .saved-meta span {
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }

  .ctx-backdrop {
    position: fixed; inset: 0; z-index: 99;
  }
  .ctx-menu {
    position: fixed; z-index: 100;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 4px;
    box-shadow: 0 4px 12px rgba(0,0,0,0.3);
    min-width: 180px;
    padding: 4px 0;
  }
  .ctx-menu button {
    display: block; width: 100%; text-align: left;
    padding: 7px 14px; background: none; border: none;
    color: var(--text); font-size: 12px; cursor: pointer;
  }
  .ctx-menu button:hover { background: var(--bg-hover, rgba(255,255,255,0.07)); }
</style>
