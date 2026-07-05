<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { activeConnections, activeTab, statusMessage } from '../stores/appStore';
  import type { DatabaseConnection } from '../stores/appStore';
  import { ListDatabaseConnections, TerminateDatabaseConnection } from '../../desktop/gen/main/App';

  export let tabId: string;

  let rows: DatabaseConnection[] = [];
  let loading = false;
  let error = '';
  let lastUpdated = '';
  let terminatingId = '';
  let confirmConnection: DatabaseConnection | null = null;
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  $: tab = $activeTab?.id === tabId ? $activeTab : null;
  $: connId = tab?.connId ?? '';
  $: connection = $activeConnections.find((entry) => entry.config.id === connId);
  $: driver = connection?.config.driver ?? '';
  $: unsupported = driver === 'sqlite';

  onMount(() => {
    void refreshConnections();
    pollTimer = setInterval(() => {
      void refreshConnections();
    }, 10_000);
  });

  onDestroy(() => {
    if (pollTimer) clearInterval(pollTimer);
  });

  async function refreshConnections() {
    if (!connId || unsupported) {
      rows = [];
      return;
    }

    loading = true;
    error = '';
    try {
      rows = await ListDatabaseConnections(connId);
      lastUpdated = new Date().toLocaleTimeString();
    } catch (e) {
      error = String(e);
      statusMessage.set(`Connection list error: ${e}`);
    } finally {
      loading = false;
    }
  }

  function requestTerminate(row: DatabaseConnection) {
    confirmConnection = row;
  }

  async function confirmTerminate() {
    if (!confirmConnection || !connId) return;

    const target = confirmConnection;
    confirmConnection = null;
    terminatingId = target.id;
    try {
      await TerminateDatabaseConnection(connId, target.id);
      statusMessage.set(`Terminated database connection ${target.id}`);
      await refreshConnections();
    } catch (e) {
      statusMessage.set(`Terminate connection error: ${e}`);
      error = String(e);
      await refreshConnections();
    } finally {
      terminatingId = '';
    }
  }

  function commandPreview(command: string) {
    const trimmed = command.trim();
    return trimmed || 'No recent command';
  }
</script>

<section class="connections-view">
  <header class="connections-toolbar">
    <div class="title-block">
      <h2>Database Connections</h2>
      <span>{connection?.config.name ?? 'Unknown connection'}</span>
    </div>
    <div class="toolbar-actions">
      {#if lastUpdated}
        <span class="updated">Updated {lastUpdated}</span>
      {/if}
      <button type="button" on:click={refreshConnections} disabled={loading || unsupported}>
        {loading ? 'Refreshing...' : 'Refresh'}
      </button>
    </div>
  </header>

  {#if unsupported}
    <div class="empty-state">
      SQLite uses local file handles rather than server-side sessions, so there are no database connections to manage.
    </div>
  {:else if error}
    <div class="error-state">{error}</div>
  {/if}

  {#if !unsupported}
    <div class="connections-table-wrap">
      <table class="connections-table">
        <thead>
          <tr>
            <th>ID</th>
            <th>User</th>
            <th>Database</th>
            <th>Client</th>
            <th>State</th>
            <th>Opened</th>
            <th>Last Active</th>
            <th>Most Recent Command</th>
            <th class="action-col">Action</th>
          </tr>
        </thead>
        <tbody>
          {#if rows.length === 0 && !loading}
            <tr>
              <td colspan="9" class="empty-row">No active database connections found.</td>
            </tr>
          {:else}
            {#each rows as row (row.id)}
              <tr>
                <td class="mono">{row.id}</td>
                <td>{row.user || 'Unknown'}</td>
                <td>{row.database || 'Unknown'}</td>
                <td>{row.client || 'Local'}</td>
                <td>{row.state || 'Unknown'}</td>
                <td>{row.openedAt || 'Unknown'}</td>
                <td>{row.lastActiveAt || 'Unknown'}</td>
                <td class="command" title={commandPreview(row.mostRecentCommand)}>
                  {commandPreview(row.mostRecentCommand)}
                </td>
                <td class="action-col">
                  <button
                    type="button"
                    class="terminate-btn"
                    disabled={!row.canTerminate || terminatingId === row.id}
                    on:click={() => requestTerminate(row)}
                    title={row.canTerminate ? 'Terminate connection' : 'Cannot terminate this management session'}
                  >
                    {terminatingId === row.id ? 'Closing...' : 'Terminate'}
                  </button>
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </div>
  {/if}
</section>

{#if confirmConnection}
  <div class="modal-overlay" role="dialog" aria-modal="true" aria-label="Terminate database connection">
    <div class="modal">
      <div class="modal-header">
        <h3>Terminate Connection?</h3>
      </div>
      <div class="modal-body">
        <p>
          Close connection {confirmConnection.id} for {confirmConnection.user || 'unknown user'}?
          Any running command on that connection may fail.
        </p>
      </div>
      <div class="modal-footer">
        <button type="button" class="btn-secondary" on:click={() => (confirmConnection = null)}>Cancel</button>
        <button type="button" class="btn-danger" on:click={confirmTerminate}>Terminate</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .connections-view {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-width: 0;
    background: var(--bg-editor);
    color: var(--text);
  }

  .connections-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-toolbar);
  }

  .title-block {
    display: flex;
    align-items: baseline;
    gap: 10px;
    min-width: 0;
  }

  h2 {
    margin: 0;
    font-size: calc(14px * var(--app-font-scale));
    font-weight: 600;
  }

  .title-block span,
  .updated {
    color: var(--text-muted);
    font-size: calc(12px * var(--app-font-scale));
    white-space: nowrap;
  }

  .toolbar-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  button {
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-input);
    color: var(--text);
    cursor: pointer;
    font-size: calc(12px * var(--app-font-scale));
    padding: 5px 10px;
  }

  button:hover:not(:disabled) {
    border-color: var(--accent);
  }

  button:disabled {
    cursor: default;
    opacity: 0.5;
  }

  .connections-table-wrap {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .connections-table {
    width: 100%;
    border-collapse: collapse;
    table-layout: fixed;
    font-size: calc(12px * var(--app-font-scale));
  }

  th,
  td {
    border-bottom: 1px solid var(--border-subtle);
    padding: 7px 8px;
    text-align: left;
    vertical-align: top;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  th {
    position: sticky;
    top: 0;
    z-index: 1;
    background: var(--bg-surface);
    color: var(--text-muted);
    font-weight: 600;
  }

  tbody tr:nth-child(even) {
    background: var(--bg-row-alt);
  }

  .mono {
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  }

  .command {
    white-space: nowrap;
  }

  .action-col {
    width: 108px;
    text-align: right;
  }

  .terminate-btn {
    color: var(--error);
  }

  .terminate-btn:hover:not(:disabled) {
    background: rgba(248, 113, 113, 0.1);
    border-color: var(--error);
  }

  .empty-row,
  .empty-state,
  .error-state {
    color: var(--text-muted);
    padding: 14px;
  }

  .error-state {
    color: var(--error);
    border-bottom: 1px solid var(--border);
  }

  .modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 500;
  }

  .modal {
    width: 420px;
    max-width: 94vw;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
  }

  .modal-header,
  .modal-body,
  .modal-footer {
    padding: 16px 18px;
  }

  .modal-header {
    border-bottom: 1px solid var(--border);
  }

  .modal-header h3 {
    margin: 0;
    font-size: calc(15px * var(--app-font-scale));
  }

  .modal-body p {
    margin: 0;
    color: var(--text-dim);
    line-height: 1.4;
  }

  .modal-footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    border-top: 1px solid var(--border);
  }

  .btn-danger {
    background: rgba(248, 113, 113, 0.16);
    border-color: rgba(248, 113, 113, 0.45);
    color: var(--error);
  }
</style>
