<script lang="ts">
  import { showConnectionDialog, editingConnection, tabs, activeTabId, activeConnections, selectedConnId } from '../stores/appStore';
  import { get } from 'svelte/store';

  function newTab() {
    const connId = get(selectedConnId);
    tabs.add(connId);
    const allTabs = get(tabs);
    activeTabId.set(allTabs[allTabs.length - 1].id);
  }

  function newConnection() {
    editingConnection.set(null);
    showConnectionDialog.set(true);
  }
</script>

<header class="toolbar">
  <div class="toolbar-left">
    <span class="app-name">multidb</span>
  </div>

  <div class="toolbar-center">
    <button class="tb-btn" on:click={newConnection} title="New Connection">
      🔌 <span>New Connection</span>
    </button>
    <button class="tb-btn" on:click={newTab} title="Open new SQL tab">
      + <span>New Query</span>
    </button>
  </div>

  <div class="toolbar-right">
    {#if $activeConnections.length > 0}
      <select
        class="conn-select"
        bind:value={$selectedConnId}
      >
        <option value="">— connection —</option>
        {#each $activeConnections as conn}
          <option value={conn.config.id}>
            {conn.config.name} ({conn.config.driver})
          </option>
        {/each}
      </select>
    {/if}
  </div>
</header>

<style>
  .toolbar {
    display: flex; align-items: center;
    padding: 0 12px;
    height: 40px;
    background: var(--bg-toolbar);
    border-bottom: 1px solid var(--border);
    gap: 8px;
    flex-shrink: 0;
  }
  .toolbar-left { display: flex; align-items: center; }
  .app-name { font-weight: 700; font-size: 14px; color: var(--accent); letter-spacing: -0.5px; }
  .toolbar-center { display: flex; align-items: center; gap: 4px; flex: 1; justify-content: center; }
  .toolbar-right { display: flex; align-items: center; gap: 8px; margin-left: auto; }
  .tb-btn {
    display: flex; align-items: center; gap: 5px;
    background: var(--bg-input); border: 1px solid var(--border);
    color: var(--text); padding: 4px 12px; border-radius: 4px;
    font-size: 12px; cursor: pointer;
  }
  .tb-btn:hover { border-color: var(--accent); }
  .conn-select {
    background: var(--bg-input); border: 1px solid var(--border);
    color: var(--text); padding: 4px 8px; border-radius: 4px;
    font-size: 12px; max-width: 200px;
  }
  .conn-select:focus { outline: none; border-color: var(--accent); }
</style>
