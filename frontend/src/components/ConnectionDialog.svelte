<script lang="ts">
  import { showConnectionDialog, editingConnection, activeConnections, selectedConnId, statusMessage, tabs, refreshConnectionSchema, activeServerGroupId, addConnectionToGroup } from '../stores/appStore';
  import type { ConnectionConfig } from '../stores/appStore';
  import { SaveAndConnect, SelectSqliteFile, TestConnection } from '../../desktop/gen/main/App';

  let form: ConnectionConfig = emptyForm();
  let testing = false;
  let saving = false;
  let testResult = '';
  let testError = '';
  let awsIamSelected = false;
  const DEFAULT_TAB_COLOR = '#6366f1';

  $: awsIamSelected = form.driver === 'mysql' && isAwsIamMode(form.authMode);

  $: if ($showConnectionDialog) {
    form = $editingConnection
      ? {
          ...emptyForm(),
          ...$editingConnection,
          tabColor: $editingConnection.tabColor ?? '',
          tabTextBlack: !!$editingConnection.tabTextBlack,
          hasSavedPassword: !!$editingConnection.hasSavedPassword,
          authMode: isAwsIamMode($editingConnection.authMode)
            ? 'awsIam'
            : ($editingConnection.authMode || 'password'),
          awsRegion: $editingConnection.awsRegion ?? '',
          awsProfile: $editingConnection.awsProfile ?? '',
          sslCaPath: $editingConnection.sslCaPath ?? '',
        }
      : emptyForm();
    testResult = '';
    testError = '';
  }

  function isValidHexColor(value: string): boolean {
    return /^#[0-9a-fA-F]{6}$/.test(value);
  }

  function normalizeHexColor(value: string): string {
    return isValidHexColor(value) ? value.toLowerCase() : '';
  }

  function getPickerColor(): string {
    const normalized = normalizeHexColor(form.tabColor ?? '');
    return normalized || DEFAULT_TAB_COLOR;
  }

  function isAwsIamMode(mode: string | null | undefined): boolean {
    const normalized = (mode ?? '').replace(/[_\s-]/g, '').toLowerCase();
    return normalized === 'awsiam';
  }

  function usesAwsIamAuth(value: ConnectionConfig = form): boolean {
    return value.driver === 'mysql' && isAwsIamMode(value.authMode);
  }

  function emptyForm(): ConnectionConfig {
    return {
      id: crypto.randomUUID(),
      name: '',
      driver: 'mysql',
      tabColor: '',
      tabTextBlack: false,
      host: 'localhost',
      port: 3306,
      username: '',
      password: '',
      hasSavedPassword: false,
      authMode: 'password',
      database: '',
      dsn: '',
      awsRegion: '',
      awsProfile: '',
      sslCaPath: '',
      useKubePortForward: false,
      kubeContext: '',
      kubeNamespace: '',
      kubeResource: '',
      kubeLocalPort: 0,
      kubeRemotePort: 0,
    };
  }

  function onTabColorPick() {
    form.tabColor = (form.tabColor ?? '').trim();
  }

  function onPickerInput(e: Event) {
    form.tabColor = (e.target as HTMLInputElement).value;
  }

  function clearTabColor() {
    form.tabColor = '';
  }

  function driverDefaultPort(driver: string): number {
    if (driver === 'mysql') return 3306;
    if (driver === 'postgres') return 5432;
    return 0;
  }

  function onDriverChange() {
    form.port = driverDefaultPort(form.driver);
    if (form.driver !== 'mysql') {
      form.authMode = 'password';
      form.password = '';
      form.hasSavedPassword = false;
    }
  }

  function onAuthModeChange(event: Event) {
    const nextMode = (event.currentTarget as HTMLSelectElement).value;
    form.authMode = isAwsIamMode(nextMode) ? 'awsIam' : 'password';
    if (awsIamSelected) {
      form.password = '';
      form.hasSavedPassword = false;
      form.useKubePortForward = false;
    }
  }

  function onKubeToggle() {
    if (form.useKubePortForward) {
      if (!form.kubeRemotePort) form.kubeRemotePort = form.port;
      if (!form.kubeLocalPort) form.kubeLocalPort = form.port;
    }
  }

  function inferConnectionName(path: string): string {
    const filename = path.split(/[\\/]/).pop() ?? path;
    const trimmed = filename.trim();
    return trimmed.replace(/\.(sqlite|sqlite3|db|db3)$/i, '') || trimmed;
  }

  function validateForm(requireName: boolean): string {
    if (requireName && !form.name.trim()) return 'Name is required';
    if (form.driver === 'sqlite' && !form.database.trim()) {
      return 'SQLite database path is required';
    }
    if (form.driver !== 'sqlite') {
      if (!form.host.trim()) return 'Host is required';
      if (!form.username.trim()) return 'Username is required';
      if (!form.database.trim()) return 'Database is required';
    }
    if (awsIamSelected) {
      if (!form.awsRegion.trim()) return 'AWS region is required for IAM authentication';
      if (form.useKubePortForward) {
        return 'AWS IAM authentication is not supported with Kubernetes port forwarding';
      }
    }
    return '';
  }

  async function browseSqliteFile() {
    testResult = '';
    testError = '';
    try {
      const selected = await SelectSqliteFile();
      if (!selected) return;

      form.database = selected;
      if (!form.name.trim()) {
        form.name = inferConnectionName(selected);
      }
    } catch (e: any) {
      testError = String(e);
    }
  }

  function connectionTargetChanged(previous: ConnectionConfig | null, next: ConnectionConfig): boolean {
    if (!previous) return true;
    const keys: (keyof ConnectionConfig)[] = [
      'driver',
      'host',
      'port',
      'username',
      'password',
      'authMode',
      'database',
      'dsn',
      'awsRegion',
      'awsProfile',
      'sslCaPath',
      'useKubePortForward',
      'kubeContext',
      'kubeNamespace',
      'kubeResource',
      'kubeLocalPort',
      'kubeRemotePort',
    ];
    return keys.some((key) => previous[key] !== next[key]);
  }

  async function handleTest() {
    testResult = '';
    testError = '';
    const validationError = validateForm(false);
    if (validationError) {
      testError = validationError;
      return;
    }
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
    const validationError = validateForm(true);
    if (validationError) { testError = validationError; return; }
    saving = true;
    testError = '';
    try {
      const previous = $editingConnection ? { ...$editingConnection } : null;
      const shouldRefreshSchema = connectionTargetChanged(previous, form);
      await SaveAndConnect(form);
      activeConnections.update(conns => {
        const exists = conns.find(c => c.config.id === form.id);
        if (exists) {
          return conns.map(c => c.config.id === form.id
            ? {
                config: { ...form },
                schema: shouldRefreshSchema ? null : c.schema,
                schemaLoading: shouldRefreshSchema ? false : c.schemaLoading,
                schemaError: shouldRefreshSchema ? null : c.schemaError,
              }
            : c);
        }
        return [...conns, { config: { ...form }, schema: null, schemaLoading: false, schemaError: null }];
      });
      // Force re-render of tabs for this connection so custom tab styling updates immediately.
      tabs.set($tabs.map(t => t.connId === form.id ? { ...t } : t));
      if (!$selectedConnId) selectedConnId.set(form.id);
      if (!$editingConnection && $activeServerGroupId) {
        addConnectionToGroup(form.id, $activeServerGroupId);
      }
      statusMessage.set(`Connected to ${form.name}`);
      if (shouldRefreshSchema) void refreshConnectionSchema(form.id);
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
<div class="modal-overlay" role="dialog" aria-modal="true" aria-label="Connection Manager">
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
      <div class="form-row tab-color-row">
        <label class="tab-color-label">Tab Colour
          <div class="tab-color-inputs">
            <input
              type="text"
              bind:value={form.tabColor}
              placeholder="Default"
              spellcheck="false"
            />
            <input
              class="tab-color-picker"
              type="color"
              value={getPickerColor()}
              on:input={onPickerInput}
              on:change={onPickerInput}
              aria-label="Pick tab colour"
            />
            <button type="button" class="btn-clear-color" on:click={clearTabColor}>Clear</button>
          </div>
        </label>
        <label class="checkbox-label tab-black-text-label">
          <input type="checkbox" bind:checked={form.tabTextBlack} />
          Black text
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

      {#if form.driver === 'mysql'}
      <div class="form-row">
        <label>Authentication
          <select bind:value={form.authMode} on:change={(event) => onAuthModeChange(event)}>
            <option value="password">Password</option>
            <option value="awsIam">AWS IAM</option>
          </select>
        </label>
      </div>
      {/if}

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
        {#if awsIamSelected}
        <label>AWS Region
          <input type="text" bind:value={form.awsRegion} placeholder="us-east-1" spellcheck="false" />
        </label>
        <label>AWS Profile
          <input type="text" bind:value={form.awsProfile} placeholder="default" spellcheck="false" />
        </label>
        {:else}
        <label>Username
          <input
            type="text"
            bind:value={form.username}
            autocomplete="off"
            autocapitalize="none"
            spellcheck="false"
            inputmode="text"
          />
        </label>
        <label>Password
          <input type="password" bind:value={form.password} autocomplete="new-password" />
        </label>
        {/if}
      </div>
      {#if awsIamSelected}
      <div class="form-row">
        <label>TLS CA Bundle Path
          <input type="text" bind:value={form.sslCaPath} placeholder="Optional PEM bundle" spellcheck="false" />
        </label>
      </div>
      <div class="form-row">
        <label>Database User
          <input
            type="text"
            bind:value={form.username}
            autocomplete="off"
            autocapitalize="none"
            spellcheck="false"
            inputmode="text"
            placeholder="db_user"
          />
        </label>
      </div>
      {/if}
      <div class="form-row">
        <label>Database
          <input type="text" bind:value={form.database} placeholder="my_db" />
        </label>
      </div>
      {:else}
      <div class="form-row">
        <label>Database File Path
          <div class="sqlite-file-row">
            <input type="text" bind:value={form.database} placeholder="C:\\path\\to\\file.db or :memory:" />
            <button type="button" class="btn-secondary btn-browse" on:click={browseSqliteFile}>
              Browse…
            </button>
          </div>
        </label>
        <span class="field-help">Choose an existing SQLite database file, or enter <code>:memory:</code> for an in-memory database.</span>
      </div>
      {/if}

      {#if form.driver !== 'sqlite'}
      <div class="form-row">
        <label class="checkbox-label">
          <input type="checkbox" bind:checked={form.useKubePortForward} on:change={onKubeToggle} disabled={awsIamSelected} />
          Use Kubernetes port forwarding
        </label>
      </div>
      {#if form.useKubePortForward}
      <div class="kube-section">
        <div class="form-row two-col">
          <label>Context
            <input type="text" bind:value={form.kubeContext} placeholder="my-cluster" />
          </label>
          <label>Namespace
            <input type="text" bind:value={form.kubeNamespace} placeholder="default" />
          </label>
        </div>
        <div class="form-row">
          <label>Target
            <input type="text" bind:value={form.kubeResource} placeholder="service/postgres" />
          </label>
        </div>
        <div class="form-row two-col">
          <label>Local Port
            <input type="number" bind:value={form.kubeLocalPort} min="1" max="65535" />
          </label>
          <label>Remote Port
            <input type="number" bind:value={form.kubeRemotePort} min="1" max="65535" />
          </label>
        </div>
      </div>
      {/if}
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
  .modal-header h2 { margin: 0; font-size: calc(16px * var(--app-font-scale)); font-weight: 600; }
  .close-btn {
    background: none; border: none; color: var(--text-muted);
    cursor: pointer; font-size: calc(16px * var(--app-font-scale)); padding: 4px 8px;
  }
  .close-btn:hover { color: var(--text); }
  .modal-body { padding: 20px; display: flex; flex-direction: column; gap: 12px; }
  .form-row { display: flex; flex-direction: column; gap: 4px; }
  .tab-color-row { gap: 8px; }
  .form-row.two-col { flex-direction: row; gap: 12px; }
  .form-row.two-col label { flex: 1; }
  label { display: flex; flex-direction: column; gap: 4px; font-size: calc(12px * var(--app-font-scale)); color: var(--text-muted); }
  .tab-color-label { gap: 6px; }
  .tab-color-inputs { display: flex; align-items: center; gap: 8px; }
  .sqlite-file-row { display: flex; align-items: center; gap: 8px; }
  .sqlite-file-row input { flex: 1; }
  .tab-color-picker {
    width: 36px;
    min-width: 36px;
    height: 32px;
    padding: 0;
    border-radius: 4px;
    cursor: pointer;
  }
  .btn-clear-color {
    height: 32px;
    padding: 0 10px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-input);
    color: var(--text);
    font-size: calc(12px * var(--app-font-scale));
    cursor: pointer;
  }
  .btn-clear-color:hover { border-color: var(--accent); }
  .btn-browse {
    flex: 0 0 auto;
    white-space: nowrap;
  }
  .tab-black-text-label { width: fit-content; }
  input, select {
    background: var(--bg-input); border: 1px solid var(--border);
    color: var(--text); padding: 7px 10px; border-radius: 4px;
    font-size: calc(13px * var(--app-font-scale)); width: 100%; box-sizing: border-box;
  }
  .field-help {
    color: var(--text-muted);
    font-size: calc(11px * var(--app-font-scale));
    line-height: 1.4;
  }
  .field-help code {
    font-family: ui-monospace, SFMono-Regular, SFMono-Regular, Consolas, monospace;
    color: var(--text);
  }
  input:focus, select:focus { outline: none; border-color: var(--accent); }
  .modal-footer {
    display: flex; justify-content: flex-end; gap: 8px;
    padding: 16px 20px; border-top: 1px solid var(--border);
  }
  .btn-primary, .btn-secondary {
    padding: 7px 16px; border-radius: 4px; font-size: calc(13px * var(--app-font-scale));
    cursor: pointer; border: 1px solid transparent;
  }
  .btn-primary { background: var(--accent); color: #fff; border-color: var(--accent); }
  .btn-primary:hover { background: var(--accent-hover); }
  .btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }
  .btn-secondary { background: var(--bg-input); color: var(--text); border-color: var(--border); }
  .btn-secondary:hover { border-color: var(--accent); }
  .btn-secondary:disabled { opacity: 0.5; cursor: not-allowed; }
  .success { color: var(--success); font-size: calc(12px * var(--app-font-scale)); margin: 0; }
  .checkbox-label {
    flex-direction: row; align-items: center; gap: 8px; cursor: pointer;
  }
  .checkbox-label input[type="checkbox"] { width: auto; cursor: pointer; }
  .kube-section {
    display: flex; flex-direction: column; gap: 12px;
    padding: 12px; border: 1px solid var(--border);
    border-radius: 4px; background: var(--bg-input);
  }
  .error { color: var(--error); font-size: calc(12px * var(--app-font-scale)); margin: 0; }
</style>
