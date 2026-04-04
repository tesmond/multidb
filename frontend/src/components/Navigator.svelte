<script lang="ts">
  import { activeConnections, selectedConnId, showConnectionDialog, editingConnection, tabs, activeTabId, statusMessage } from '../stores/appStore';
  import type { ActiveConnection } from '../stores/appStore';
  import { GetSchema, Disconnect } from '../../wailsjs/go/main/App';
  import { get } from 'svelte/store';

  // Expandable node state
  let expanded: Record<string, boolean> = {};
  let expandedTables: Record<string, boolean> = {};

  async function loadSchema(conn: ActiveConnection) {
    if (conn.schemaLoading) return;
    activeConnections.update(conns =>
      conns.map(c => c.config.id === conn.config.id ? { ...c, schemaLoading: true, schemaError: null } : c)
    );
    try {
      const tree = await GetSchema(conn.config.id);
      activeConnections.update(conns =>
        conns.map(c => c.config.id === conn.config.id ? { ...c, schema: tree, schemaLoading: false } : c)
      );
    } catch (e: any) {
      activeConnections.update(conns =>
        conns.map(c => c.config.id === conn.config.id ? { ...c, schemaLoading: false, schemaError: String(e) } : c)
      );
    }
  }

  function toggleConn(id: string, conn: ActiveConnection) {
    expanded[id] = !expanded[id];
    if (expanded[id] && !conn.schema) {
      loadSchema(conn);
    }
    expanded = { ...expanded };
    selectedConnId.set(id);
  }

  function toggleTable(tableKey: string) {
    expandedTables[tableKey] = !expandedTables[tableKey];
    expandedTables = { ...expandedTables };
  }

  function editConn(conn: ActiveConnection) {
    editingConnection.set({ ...conn.config });
    showConnectionDialog.set(true);
  }

  async function disconnectConn(id: string) {
    try {
      await Disconnect(id);
      activeConnections.update(conns => conns.filter(c => c.config.id !== id));
      const $sel = get(selectedConnId);
      if ($sel === id) selectedConnId.set('');
    } catch (e: any) {
      statusMessage.set(`Disconnect error: ${e}`);
    }
  }

  function quoteIdentifier(name: string, driver: string): string {
    return driver === 'mysql' ? `\`${name}\`` : `"${name}"`;
  }

  function qualifyTable(connId: string, tableName: string, schemaName?: string): string {
    const conn = get(activeConnections).find(c => c.config.id === connId);
    const driver = conn?.config.driver ?? 'postgres';
    if (!schemaName) return tableName;
    return `${quoteIdentifier(schemaName, driver)}.${quoteIdentifier(tableName, driver)}`;
  }

  function openTableQuery(connId: string, tableName: string, schemaName?: string) {
    const id = crypto.randomUUID();
    tabs.add(connId);
    // Find the just-added tab and set its SQL
    const allTabs = get(tabs);
    const newTab = allTabs[allTabs.length - 1];
    const qualifiedName = qualifyTable(connId, tableName, schemaName);
    tabs.updateTab(newTab.id, {
      sql: `SELECT * FROM ${qualifiedName} LIMIT 100;`,
      title: tableName,
      connId,
    });
    activeTabId.set(newTab.id);
    selectedConnId.set(connId);
  }

  function copyName(name: string) {
    navigator.clipboard.writeText(name).catch(() => {});
  }

  // Context menu state
  let contextMenu: { x: number; y: number; tableName: string; connId: string; schemaName?: string } | null = null;

  function openContextMenu(e: MouseEvent, connId: string, tableName: string, schemaName?: string) {
    e.preventDefault();
    contextMenu = { x: e.clientX, y: e.clientY, tableName, connId, schemaName };
  }

  function handleContextAction(action: 'view' | 'copy' | 'select') {
    if (!contextMenu) return;
    const { connId, tableName, schemaName } = contextMenu;
    contextMenu = null;
    if (action === 'view') openTableQuery(connId, tableName, schemaName);
    else if (action === 'copy') copyName(qualifyTable(connId, tableName, schemaName));
    else if (action === 'select') openTableQuery(connId, tableName, schemaName);
  }

  function closeContextMenu() { contextMenu = null; }
</script>

<svelte:window on:click={closeContextMenu} />

<aside class="navigator">
  <div class="nav-header">
    <span class="nav-title">Navigator</span>
    <button class="icon-btn" on:click={() => { showConnectionDialog.set(true); editingConnection.set(null); }} title="New Connection">+</button>
  </div>

  <div class="nav-content">
    {#each $activeConnections as conn (conn.config.id)}
      <div class="conn-node">
        <div
          class="conn-label"
          class:selected={$selectedConnId === conn.config.id}
          on:click={() => toggleConn(conn.config.id, conn)}
          role="treeitem" aria-selected={false}
          aria-expanded={!!expanded[conn.config.id]}
          tabindex="0"
          on:keydown={e => e.key === 'Enter' && toggleConn(conn.config.id, conn)}
        >
          <span class="chevron">{expanded[conn.config.id] ? '▾' : '▸'}</span>
          <span class="conn-icon">🔌</span>
          <span class="conn-name">{conn.config.name}</span>
          <span class="driver-badge">{conn.config.driver}</span>
          <div class="conn-actions">
            <button class="icon-btn" on:click|stopPropagation={() => editConn(conn)} title="Edit">✏️</button>
            <button class="icon-btn" on:click|stopPropagation={() => disconnectConn(conn.config.id)} title="Disconnect">✕</button>
          </div>
        </div>

        {#if expanded[conn.config.id]}
          <div class="conn-children">
            {#if conn.schemaLoading}
              <div class="nav-info">Loading schema…</div>
            {:else if conn.schemaError}
              <div class="nav-error">{conn.schemaError}</div>
            {:else if conn.schema}
              {#if conn.schema.schemas?.length}
                <!-- Postgres: schema-grouped hierarchy -->
                {#each conn.schema.schemas as pgSchema}
                  <div class="schema-section">
                    <div
                      class="section-label schema-node"
                      on:click={() => toggleTable(`${conn.config.id}-schema-${pgSchema.name}`)}
                      role="treeitem" aria-selected={false}
                      aria-expanded={!!expandedTables[`${conn.config.id}-schema-${pgSchema.name}`]}
                      tabindex="0"
                      on:keydown={e => e.key === 'Enter' && toggleTable(`${conn.config.id}-schema-${pgSchema.name}`)}
                    >
                      <span class="chevron">{expandedTables[`${conn.config.id}-schema-${pgSchema.name}`] ? '▾' : '▸'}</span>
                      <span class="table-icon">🗂</span>
                      {pgSchema.name}
                    </div>
                    {#if expandedTables[`${conn.config.id}-schema-${pgSchema.name}`]}
                      <div class="conn-children">
                        <!-- Tables -->
                        <div class="schema-section">
                          <div
                            class="section-label"
                            on:click={() => toggleTable(`${conn.config.id}-${pgSchema.name}-tables`)}
                            role="treeitem" aria-selected={false}
                            tabindex="0"
                            on:keydown={e => e.key === 'Enter' && toggleTable(`${conn.config.id}-${pgSchema.name}-tables`)}
                          >
                            <span class="chevron">{expandedTables[`${conn.config.id}-${pgSchema.name}-tables`] ? '▾' : '▸'}</span>
                            Tables <span class="count">({pgSchema.tables?.length ?? 0})</span>
                          </div>
                          {#if expandedTables[`${conn.config.id}-${pgSchema.name}-tables`]}
                            {#each pgSchema.tables ?? [] as table}
                              <div class="table-node">
                                <div
                                  class="table-label"
                                  on:click={() => toggleTable(`${conn.config.id}-${pgSchema.name}-t-${table.name}`)}
                                  on:contextmenu={e => openContextMenu(e, conn.config.id, table.name, pgSchema.name)}
                                  role="treeitem" aria-selected={false}
                                  tabindex="0"
                                  on:keydown={e => e.key === 'Enter' && toggleTable(`${conn.config.id}-${pgSchema.name}-t-${table.name}`)}
                                >
                                  <span class="chevron">{expandedTables[`${conn.config.id}-${pgSchema.name}-t-${table.name}`] ? '▾' : '▸'}</span>
                                  <span class="table-icon">📋</span>
                                  {table.name}
                                </div>
                                {#if expandedTables[`${conn.config.id}-${pgSchema.name}-t-${table.name}`]}
                                  <div class="col-list">
                                    {#each table.columns ?? [] as col}
                                      <div class="col-row">
                                        <span class="col-key" title={col.key}>{col.key === 'PRI' ? '🔑' : '·'}</span>
                                        <span class="col-name">{col.name}</span>
                                        <span class="col-type">{col.type}</span>
                                      </div>
                                    {/each}
                                  </div>
                                {/if}
                              </div>
                            {/each}
                          {/if}
                        </div>
                        <!-- Views -->
                        {#if pgSchema.views?.length > 0}
                        <div class="schema-section">
                          <div
                            class="section-label"
                            on:click={() => toggleTable(`${conn.config.id}-${pgSchema.name}-views`)}
                            role="treeitem" aria-selected={false}
                            tabindex="0"
                            on:keydown={e => e.key === 'Enter' && toggleTable(`${conn.config.id}-${pgSchema.name}-views`)}
                          >
                            <span class="chevron">{expandedTables[`${conn.config.id}-${pgSchema.name}-views`] ? '▾' : '▸'}</span>
                            Views <span class="count">({pgSchema.views.length})</span>
                          </div>
                          {#if expandedTables[`${conn.config.id}-${pgSchema.name}-views`]}
                            {#each pgSchema.views as view}
                              <div class="table-label leaf">
                                <span class="table-icon">👁</span> {view.name}
                              </div>
                            {/each}
                          {/if}
                        </div>
                        {/if}
                        <!-- Indexes -->
                        {#if pgSchema.indexes?.length > 0}
                        <div class="schema-section">
                          <div
                            class="section-label"
                            on:click={() => toggleTable(`${conn.config.id}-${pgSchema.name}-indexes`)}
                            role="treeitem" aria-selected={false}
                            tabindex="0"
                            on:keydown={e => e.key === 'Enter' && toggleTable(`${conn.config.id}-${pgSchema.name}-indexes`)}
                          >
                            <span class="chevron">{expandedTables[`${conn.config.id}-${pgSchema.name}-indexes`] ? '▾' : '▸'}</span>
                            Indexes <span class="count">({pgSchema.indexes.length})</span>
                          </div>
                          {#if expandedTables[`${conn.config.id}-${pgSchema.name}-indexes`]}
                            {#each pgSchema.indexes as idx}
                              <div class="table-label leaf">
                                <span class="table-icon">⚡</span> {idx}
                              </div>
                            {/each}
                          {/if}
                        </div>
                        {/if}
                      </div>
                    {/if}
                  </div>
                {/each}
              {:else}
              <!-- MySQL / SQLite: flat hierarchy -->
              <!-- Tables -->
              <div class="schema-section">
                <div
                  class="section-label"
                  on:click={() => toggleTable(`${conn.config.id}-tables`)}
                  role="treeitem" aria-selected={false}
                  aria-expanded={!!expandedTables[`${conn.config.id}-tables`]}
                  tabindex="0"
                  on:keydown={e => e.key === 'Enter' && toggleTable(`${conn.config.id}-tables`)}
                >
                  <span class="chevron">{expandedTables[`${conn.config.id}-tables`] ? '▾' : '▸'}</span>
                  Tables <span class="count">({conn.schema.tables?.length ?? 0})</span>
                </div>
                {#if expandedTables[`${conn.config.id}-tables`]}
                  {#each conn.schema.tables ?? [] as table}
                    <div class="table-node">
                      <div
                        class="table-label"
                        on:click={() => toggleTable(`${conn.config.id}-t-${table.name}`)}
                        on:contextmenu={e => openContextMenu(e, conn.config.id, table.name)}
                        role="treeitem" aria-selected={false}
                        tabindex="0"
                        on:keydown={e => e.key === 'Enter' && toggleTable(`${conn.config.id}-t-${table.name}`)}
                      >
                        <span class="chevron">{expandedTables[`${conn.config.id}-t-${table.name}`] ? '▾' : '▸'}</span>
                        <span class="table-icon">📋</span>
                        {table.name}
                      </div>
                      {#if expandedTables[`${conn.config.id}-t-${table.name}`]}
                        <div class="col-list">
                          {#each table.columns ?? [] as col}
                            <div class="col-row">
                              <span class="col-key" title={col.key}>{col.key === 'PRI' ? '🔑' : '·'}</span>
                              <span class="col-name">{col.name}</span>
                              <span class="col-type">{col.type}</span>
                            </div>
                          {/each}
                        </div>
                      {/if}
                    </div>
                  {/each}
                {/if}
              </div>

              <!-- Views -->
              {#if conn.schema.views?.length > 0}
              <div class="schema-section">
                <div
                  class="section-label"
                  on:click={() => toggleTable(`${conn.config.id}-views`)}
                  role="treeitem" aria-selected={false}
                  tabindex="0"
                  on:keydown={e => e.key === 'Enter' && toggleTable(`${conn.config.id}-views`)}
                >
                  <span class="chevron">{expandedTables[`${conn.config.id}-views`] ? '▾' : '▸'}</span>
                  Views <span class="count">({conn.schema.views.length})</span>
                </div>
                {#if expandedTables[`${conn.config.id}-views`]}
                  {#each conn.schema.views as view}
                    <div class="table-label leaf">
                      <span class="table-icon">👁</span> {view.name}
                    </div>
                  {/each}
                {/if}
              </div>
              {/if}

              <!-- Indexes -->
              {#if conn.schema.indexes?.length > 0}
              <div class="schema-section">
                <div
                  class="section-label"
                  on:click={() => toggleTable(`${conn.config.id}-indexes`)}
                  role="treeitem" aria-selected={false}
                  tabindex="0"
                  on:keydown={e => e.key === 'Enter' && toggleTable(`${conn.config.id}-indexes`)}
                >
                  <span class="chevron">{expandedTables[`${conn.config.id}-indexes`] ? '▾' : '▸'}</span>
                  Indexes <span class="count">({conn.schema.indexes.length})</span>
                </div>
                {#if expandedTables[`${conn.config.id}-indexes`]}
                  {#each conn.schema.indexes as idx}
                    <div class="table-label leaf">
                      <span class="table-icon">⚡</span> {idx}
                    </div>
                  {/each}
                {/if}
              </div>
              {/if}
              {/if}
            {:else}
              <div class="nav-info">Click to load schema</div>
            {/if}
          </div>
        {/if}
      </div>
    {/each}

    {#if $activeConnections.length === 0}
      <div class="empty-nav">
        <p>No connections.</p>
        <button class="btn-link" on:click={() => showConnectionDialog.set(true)}>+ New Connection</button>
      </div>
    {/if}
  </div>
</aside>

{#if contextMenu}
  <div
    class="context-menu"
    style="left:{contextMenu.x}px; top:{contextMenu.y}px"
    role="menu"
  >
    <button role="menuitem" on:click={() => handleContextAction('view')}>
      View Data (SELECT * LIMIT 100)
    </button>
    <button role="menuitem" on:click={() => handleContextAction('copy')}>
      Copy Name
    </button>
    <button role="menuitem" on:click={() => handleContextAction('select')}>
      Generate SELECT
    </button>
  </div>
{/if}

<style>
  .navigator {
    display: flex; flex-direction: column;
    height: 100%; overflow: hidden;
    background: var(--bg-panel);
    border-right: 1px solid var(--border);
    min-width: 0;
  }
  .nav-header {
    display: flex; align-items: center; justify-content: space-between;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    font-size: 11px; font-weight: 600; text-transform: uppercase; color: var(--text-muted);
  }
  .nav-title { letter-spacing: 0.05em; }
  .nav-content { flex: 1; overflow-y: auto; padding: 4px 0; }

  .conn-node { }
  .conn-label {
    display: flex; align-items: center; gap: 4px;
    padding: 5px 8px; cursor: pointer; user-select: none;
    font-size: 13px; color: var(--text);
  }
  .conn-label:hover { background: var(--bg-hover); }
  .conn-label.selected { background: var(--bg-selected); }
  .conn-label:focus { outline: 1px solid var(--accent); }
  .conn-name { font-weight: 500; flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .driver-badge {
    font-size: 10px; padding: 1px 5px; border-radius: 3px;
    background: var(--bg-badge); color: var(--text-muted);
  }
  .conn-actions { display: none; gap: 2px; align-items: center; }
  .conn-label:hover .conn-actions { display: flex; }
  .conn-icon, .table-icon { flex-shrink: 0; }

  .conn-children { padding-left: 16px; }
  .schema-section { margin: 2px 0; }
  .section-label {
    display: flex; align-items: center; gap: 4px;
    padding: 3px 8px; cursor: pointer; user-select: none;
    font-size: 12px; color: var(--text-muted); font-weight: 500;
  }
  .section-label:hover { color: var(--text); }
  .section-label.schema-node { color: var(--text); font-size: 13px; }
  .count { font-weight: 400; opacity: 0.7; }

  .table-node { }
  .table-label {
    display: flex; align-items: center; gap: 4px;
    padding: 3px 8px; cursor: pointer; user-select: none;
    font-size: 12px; color: var(--text);
  }
  .table-label:hover { background: var(--bg-hover); }
  .table-label.leaf { padding-left: 24px; }

  .col-list { padding-left: 24px; }
  .col-row {
    display: flex; align-items: center; gap: 6px;
    padding: 2px 8px; font-size: 11px; color: var(--text-muted);
  }
  .col-key { width: 14px; flex-shrink: 0; }
  .col-name { flex: 1; }
  .col-type { opacity: 0.6; font-style: italic; }

  .chevron { opacity: 0.5; font-size: 10px; width: 10px; flex-shrink: 0; }
  .icon-btn {
    background: none; border: none; cursor: pointer;
    color: var(--text-muted); font-size: 12px; padding: 2px 4px;
    line-height: 1;
  }
  .icon-btn:hover { color: var(--text); }

  .nav-info { padding: 6px 12px; font-size: 12px; color: var(--text-muted); }
  .nav-error { padding: 6px 12px; font-size: 12px; color: var(--error); }

  .empty-nav { padding: 20px 16px; text-align: center; }
  .empty-nav p { font-size: 12px; color: var(--text-muted); margin: 0 0 8px; }
  .btn-link { background: none; border: none; color: var(--accent); cursor: pointer; font-size: 12px; }
  .btn-link:hover { text-decoration: underline; }

  .context-menu {
    position: fixed; z-index: 300;
    background: var(--bg-surface); border: 1px solid var(--border);
    border-radius: 4px; min-width: 180px;
    box-shadow: 0 4px 16px rgba(0,0,0,0.4);
    overflow: hidden;
  }
  .context-menu button {
    display: block; width: 100%; text-align: left;
    padding: 8px 16px; background: none; border: none;
    color: var(--text); font-size: 13px; cursor: pointer;
  }
  .context-menu button:hover { background: var(--bg-hover); }
</style>
