<script lang="ts">
  import { showConnectionDialog, editingConnection, activeConnections, selectedConnId, statusMessage } from '../stores/appStore';
  import type { ConnectionConfig } from '../stores/appStore';
  import { SaveAndConnect, TestConnection } from '../../wailsjs/go/main/App';

  let form: ConnectionConfig = emptyForm();
  let testing = false;
  let saving = false;
  let testResult = '';
  let testError = '';

  $: if ($showConnectionDialog) {
    form = $editingConnection ? { ...$editingConnection } : emptyForm();
    testResult = '';
    testError = '';
  }

  function emptyForm(): ConnectionConfig {
    return { id: crypto.randomUUID(), name: '', driver: 'mysql', host: 'localhost', port: 3306, username: '', password: '', database: '', dsn: '' };
  }

  function driverDefaultPort(driver: string): number {
    if (driver === 'mysql') return 3306;
    if (driver === 'postgres') return 5432;
    return 0;
  }

  function onDriverChange() {
    form.port = driverDefaultPort(form.driver);
  }

  async function handleTest() {
    testResult = '';
    testError = '';
    testing = true;
    try {
      await TestConnection(form);
      testResult = 'Connection successful!';
    } catch (e: any) {
      testError = String(e);
    } finally {
      testing = false;
    }
  }

  async function handleSave() {
    if (!form.name) { testError = 'Name is required'; return; }
    saving = true;
    testError = '';
    try {
      await SaveAndConnect(form);
      activeConnections.update(conns => {
        const exists = conns.find(c => c.config.id === form.id);
        if (exists) {
          return conns.map(c => c.config.id === form.id ? { config: form, schema: null, schemaLoading: false, schemaError: null } : c);
        }
        return [...conns, { config: { ...form }, schema: null, schemaLoading: false, schemaError: null }];
      });
      if (!$selectedConnId) selectedConnId.set(form.id);
      statusMessage.set(`Connected to ${form.name}`);
      showConnectionDialog.set(false);
    } catch (e: any) {
      testError = String(e);
    } finally {
      saving = false;
    }
  }

  function close() {
    showConnectionDialog.set(false);
    editingConnection.set(null);
  }
</script>

{#if $showConnectionDialog}
<div class="modal-overlay" on:click|self={close} on:keydown={e => e.key === 'Escape' && close()} role="dialog" aria-modal="true" aria-label="Connection Manager">
  <div class="modal">
    <div class="modal-header">
      <h2>{$editingConnection ? 'Edit Connection' : 'New Connection'}</h2>
      <button class="close-btn" on:click={close} aria-label="Close">✕</button>
    </div>

    <div class="modal-body">
      <div class="form-row">
        <label>Connection Name
          <input type="text" bind:value={form.name} placeholder="My Database" />
        </label>
      </div>
      <div class="form-row">
        <label>Driver
          <select bind:value={form.driver} on:change={onDriverChange}>
            <option value="mysql">MySQL</option>
            <option value="postgres">PostgreSQL</option>
            <option value="sqlite">SQLite</option>
          </select>
        </label>
      </div>

      {#if form.driver !== 'sqlite'}
      <div class="form-row two-col">
        <label>Host
          <input type="text" bind:value={form.host} placeholder="localhost" />
        </label>
        <label>Port
          <input type="number" bind:value={form.port} min="1" max="65535" />
        </label>
      </div>
      <div class="form-row two-col">
        <label>Username
          <input type="text" bind:value={form.username} autocomplete="off" />
        </label>
        <label>Password
          <input type="password" bind:value={form.password} autocomplete="new-password" />
        </label>
      </div>
      <div class="form-row">
        <label>Database
          <input type="text" bind:value={form.database} placeholder="my_db" />
        </label>
      </div>
      {:else}
      <div class="form-row">
        <label>Database File Path
          <input type="text" bind:value={form.database} placeholder="/path/to/file.db" />
        </label>
      </div>
      {/if}

      {#if testResult}
        <p class="success">{testResult}</p>
      {/if}
      {#if testError}
        <p class="error">{testError}</p>
      {/if}
    </div>

    <div class="modal-footer">
      <button class="btn-secondary" on:click={handleTest} disabled={testing}>
        {testing ? 'Testing…' : 'Test Connection'}
      </button>
      <button class="btn-primary" on:click={handleSave} disabled={saving}>
        {saving ? 'Saving…' : 'Save & Connect'}
      </button>
    </div>
  </div>
</div>
{/if}

<style>
  .modal-overlay {
    position: fixed; inset: 0;
    background: rgba(0,0,0,0.6);
    display: flex; align-items: center; justify-content: center;
    z-index: 200;
  }
  .modal {
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    width: 480px;
    max-width: 95vw;
    box-shadow: 0 8px 32px rgba(0,0,0,0.5);
  }
  .modal-header {
    display: flex; align-items: center; justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border);
  }
  .modal-header h2 { margin: 0; font-size: 16px; font-weight: 600; }
  .close-btn {
    background: none; border: none; color: var(--text-muted);
    cursor: pointer; font-size: 16px; padding: 4px 8px;
  }
  .close-btn:hover { color: var(--text); }
  .modal-body { padding: 20px; display: flex; flex-direction: column; gap: 12px; }
  .form-row { display: flex; flex-direction: column; gap: 4px; }
  .form-row.two-col { flex-direction: row; gap: 12px; }
  .form-row.two-col label { flex: 1; }
  label { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--text-muted); }
  input, select {
    background: var(--bg-input); border: 1px solid var(--border);
    color: var(--text); padding: 7px 10px; border-radius: 4px;
    font-size: 13px; width: 100%; box-sizing: border-box;
  }
  input:focus, select:focus { outline: none; border-color: var(--accent); }
  .modal-footer {
    display: flex; justify-content: flex-end; gap: 8px;
    padding: 16px 20px; border-top: 1px solid var(--border);
  }
  .btn-primary, .btn-secondary {
    padding: 7px 16px; border-radius: 4px; font-size: 13px;
    cursor: pointer; border: 1px solid transparent;
  }
  .btn-primary { background: var(--accent); color: #fff; border-color: var(--accent); }
  .btn-primary:hover { background: var(--accent-hover); }
  .btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }
  .btn-secondary { background: var(--bg-input); color: var(--text); border-color: var(--border); }
  .btn-secondary:hover { border-color: var(--accent); }
  .btn-secondary:disabled { opacity: 0.5; cursor: not-allowed; }
  .success { color: var(--success); font-size: 12px; margin: 0; }
  .error { color: var(--error); font-size: 12px; margin: 0; }
</style>
