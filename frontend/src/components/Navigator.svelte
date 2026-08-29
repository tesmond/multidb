<script lang="ts">
  import { activeConnections, selectedConnId, showConnectionDialog, editingConnection, showImportDialog, importDialogConnId, tabs, activeTabId, statusMessage, schemaRefreshSignal, refreshConnectionSchema, loadCachedSchema, deleteCachedSchema, serverGroups, activeServerGroupId, fontScalePercent, setFontScalePercent, addServerGroup, addConnectionToGroup, removeConnectionFromGroups, moveConnectionInList, moveServerGroup, showRelationshipDiagramForConnection, showDatabaseConnectionsForConnection, openQueryTabForConnection, buildConnectionDisplayStructure } from '../stores/appStore';
  import type { ActiveConnection, SchemaTree, ServerGroup } from '../stores/appStore';
  import { CancelTestConnection, Disconnect, TestConnection, BackupTable, DropTable } from '../../desktop/gen/main/App';
  import { get } from 'svelte/store';

  // Expandable node state
  let expanded: Record<string, boolean> = {};
  let expandedTables: Record<string, boolean> = {};
  let expandedGroups: Record<string, boolean> = {};
  let addMenuOpen = false;
  let settingsOpen = false;
  let fontScaleInput = '100';
  let draggingConnId = '';
  let draggingGroupId = '';
  let dropTargetConnId = '';
  let dropTargetGroupId = '';
  let dropPlacement: 'before' | 'after' = 'before';
  let dragCandidate:
    | { kind: 'conn'; id: string; startX: number; startY: number }
    | { kind: 'group'; id: string; startX: number; startY: number }
    | null = null;
  let pointerDragging = false;
  let suppressNextClick = false;
  let showServerGroupDialog = false;
  let serverGroupTitle = '';
  let serverGroupError = '';
  let tableFilter = '';
  let normalizedTableFilter = '';
  let hasTableFilter = false;
  let lastFilterSchemaLoadKey = '';

  type SchemaTable = NonNullable<SchemaTree['tables']>[number];

  type NavItem =
    | { kind: 'group'; group: ServerGroup }
    | { kind: 'conn'; conn: ActiveConnection; groupId?: string };

  let navItems: NavItem[] = [];

  $: fontScaleInput = String($fontScalePercent);
  $: normalizedTableFilter = tableFilter.trim().toLowerCase();
  $: hasTableFilter = normalizedTableFilter.length > 0;

  $: {
    const items: NavItem[] = [];
    const filterText = normalizedTableFilter;
    const filtering = filterText.length > 0;
    const structure = buildConnectionDisplayStructure($activeConnections, $serverGroups);

    for (const { group, connections } of structure.groups) {
      const groupConns = connections
        .filter((conn) => connectionMatchesFilter(conn, filterText));

      if (!filtering || groupConns.length > 0) {
        items.push({ kind: 'group', group });
      }

      if (expandedGroups[group.id] || filtering) {
        for (const conn of groupConns) items.push({ kind: 'conn', conn, groupId: group.id });
      }
    }

    for (const conn of structure.ungrouped) {
      if (connectionMatchesFilter(conn, filterText)) {
        items.push({ kind: 'conn', conn });
      }
    }

    navItems = items;
  }

  $: {
    const missingSchemaIds = hasTableFilter
      ? $activeConnections
          .filter((conn) => !conn.schema && !conn.schemaLoading)
          .map((conn) => conn.config.id)
          .sort()
      : [];
    const loadKey = missingSchemaIds.join('|');
    if (loadKey && loadKey !== lastFilterSchemaLoadKey) {
      lastFilterSchemaLoadKey = loadKey;
      void loadCachedSchemasForFilter(missingSchemaIds);
    } else if (!hasTableFilter) {
      lastFilterSchemaLoadKey = '';
    }
  }

  function toggleConn(id: string, conn: ActiveConnection) {
    expanded[id] = !expanded[id];
    if (expanded[id] && !conn.schema) {
      void (async () => {
        await loadCachedSchema(conn.config.id);
        const current = get(activeConnections).find(c => c.config.id === conn.config.id);
        if (!current?.schema) {
          await refreshConnectionSchema(conn.config.id);
        }
      })();
    }
    expanded = { ...expanded };
    selectedConnId.set(id);
  }

  function toggleTable(tableKey: string) {
    expandedTables[tableKey] = !expandedTables[tableKey];
    expandedTables = { ...expandedTables };
  }

  function toggleGroup(groupId: string) {
    expandedGroups[groupId] = !expandedGroups[groupId];
    expandedGroups = { ...expandedGroups };
    activeServerGroupId.set(expandedGroups[groupId] ? groupId : '');
  }

  function editConn(conn: ActiveConnection) {
    editingConnection.set({ ...conn.config });
    showConnectionDialog.set(true);
  }

  function getConnectionColorSwatch(tabColor: string | undefined): string {
    if (typeof tabColor !== 'string') return 'transparent';
    return /^#[0-9a-fA-F]{6}$/.test(tabColor.trim()) ? tabColor.trim().toLowerCase() : 'transparent';
  }

  function formatBytes(bytes: number | undefined | null): string {
    if (!bytes || bytes <= 0) return '';
    const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
    let value = bytes;
    let unitIndex = 0;
    while (value >= 1024 && unitIndex < units.length - 1) {
      value /= 1024;
      unitIndex += 1;
    }
    const precision = value >= 10 || unitIndex === 0 ? 0 : 1;
    return `${value.toFixed(precision)} ${units[unitIndex]}`;
  }

  function matchesTableFilter(table: SchemaTable, filterText = normalizedTableFilter): boolean {
    if (!filterText) return true;
    if (table.name.toLowerCase().includes(filterText)) return true;
    return (table.columns ?? []).some((col) =>
      col.name.toLowerCase().includes(filterText),
    );
  }

  function filterTables(tables: SchemaTable[] | undefined | null, filterText = normalizedTableFilter): SchemaTable[] {
    const list = tables ?? [];
    return filterText ? list.filter((table) => matchesTableFilter(table, filterText)) : list;
  }

  function schemaHasMatchingTables(schema: SchemaTree, filterText = normalizedTableFilter): boolean {
    if (schema.schemas?.length) {
      return schema.schemas.some((pgSchema) => filterTables(pgSchema.tables, filterText).length > 0);
    }
    return filterTables(schema.tables, filterText).length > 0;
  }

  function connectionMatchesFilter(conn: ActiveConnection, filterText = normalizedTableFilter): boolean {
    return !filterText || (!!conn.schema && schemaHasMatchingTables(conn.schema, filterText));
  }

  function groupConnectionCount(group: ServerGroup): number {
    const connIds = new Set($activeConnections.map((conn) => conn.config.id));
    const connById = new Map($activeConnections.map((conn) => [conn.config.id, conn]));
    return group.connectionIds.filter((id) => {
      const conn = connById.get(id);
      return conn && connIds.has(id) && connectionMatchesFilter(conn);
    }).length;
  }

  async function loadCachedSchemasForFilter(connIds: string[]) {
    await Promise.all(connIds.map((connId) => loadCachedSchema(connId)));
  }

  function clearTableFilter() {
    tableFilter = '';
    normalizedTableFilter = '';
    hasTableFilter = false;
    lastFilterSchemaLoadKey = '';
  }

  function updateTableFilter(e: Event) {
    const nextValue = (e.currentTarget as HTMLInputElement).value;
    if (!nextValue) {
      clearTableFilter();
      return;
    }
    tableFilter = nextValue;
  }

  async function disconnectConn(id: string) {
    try {
      await Disconnect(id);
      // Clean up frontend state
      activeConnections.update(conns => conns.filter(c => c.config.id !== id));
      removeConnectionFromGroups(id);
      await deleteCachedSchema(id);
      tabs.closeTabsForConn(id);
      const $sel = get(selectedConnId);
      if ($sel === id) selectedConnId.set('');
    } catch (e: any) {
      statusMessage.set(`Disconnect error: ${e}`);
    }
  }

  // Test the saved connection config without opening a new one
  let testingConnId: string | null = null;
  let activeTestId = '';
  let stopRequested = false;
  async function testConn(conn: ActiveConnection) {
    const testId = crypto.randomUUID();
    testingConnId = conn.config.id;
    activeTestId = testId;
    stopRequested = false;
    statusMessage.set('Testing connection…');
    try {
      await TestConnection(conn.config, testId);
      statusMessage.set(`Connection to ${conn.config.name} succeeded ✓`);
    } catch (e: any) {
      statusMessage.set(stopRequested ? 'Connection test cancelled' : `Connection failed: ${e}`);
    } finally {
      if (activeTestId === testId) activeTestId = '';
      testingConnId = null;
    }
  }

  async function cancelTest() {
    if (!activeTestId) return;
    stopRequested = true;
    try {
      await CancelTestConnection(activeTestId);
    } catch (e: any) {
      statusMessage.set(String(e));
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

  function openNewConnection() {
    editingConnection.set(null);
    showConnectionDialog.set(true);
    addMenuOpen = false;
  }

  function createServerGroup() {
    serverGroupTitle = '';
    serverGroupError = '';
    showServerGroupDialog = true;
    addMenuOpen = false;
  }

  function closeServerGroupDialog() {
    showServerGroupDialog = false;
    serverGroupTitle = '';
    serverGroupError = '';
  }

  function saveServerGroup() {
    const title = serverGroupTitle.trim();
    if (!title) {
      serverGroupError = 'Title is required';
      return;
    }

    const groupId = addServerGroup(title);
    expandedGroups[groupId] = true;
    expandedGroups = { ...expandedGroups };
    closeServerGroupDialog();
  }

  function closeFloatingUi() {
    contextMenu = null;
    addMenuOpen = false;
    settingsOpen = false;
  }

  function adjustFontScale(delta: number) {
    setFontScalePercent($fontScalePercent + delta);
  }

  function commitFontScale() {
    setFontScalePercent(Number(fontScaleInput));
  }

  function shouldIgnorePointerDown(target: EventTarget | null) {
    const element = target as HTMLElement | null;
    return !!element?.closest('button, input, textarea, select, a, .conn-actions, .icon-btn, .header-menu, .context-menu');
  }

  function beginConnPointerDrag(e: MouseEvent, connId: string) {
    if (e.button !== 0 || shouldIgnorePointerDown(e.target)) return;
    dragCandidate = { kind: 'conn', id: connId, startX: e.clientX, startY: e.clientY };
  }

  function beginGroupPointerDrag(e: MouseEvent, groupId: string) {
    if (e.button !== 0 || shouldIgnorePointerDown(e.target)) return;
    dragCandidate = { kind: 'group', id: groupId, startX: e.clientX, startY: e.clientY };
  }

  function clearDropTargets() {
    dropTargetConnId = '';
    dropTargetGroupId = '';
  }

  function updateDropPlacement(element: HTMLElement) {
    const rect = element.getBoundingClientRect();
    dropPlacement = window.event instanceof MouseEvent && window.event.clientY > rect.top + rect.height / 2 ? 'after' : 'before';
  }

  function updatePointerDropTarget(clientX: number, clientY: number) {
    clearDropTargets();
    const target = document.elementFromPoint(clientX, clientY) as HTMLElement | null;
    if (!target) return;

    const connLabel = target.closest('.conn-label') as HTMLElement | null;
    if (draggingConnId && connLabel) {
      const connId = connLabel.dataset.connId ?? '';
      if (connId && connId !== draggingConnId) {
        const rect = connLabel.getBoundingClientRect();
        dropPlacement = clientY > rect.top + rect.height / 2 ? 'after' : 'before';
        dropTargetConnId = connId;
      }
      return;
    }

    const groupLabel = target.closest('.group-label') as HTMLElement | null;
    if (groupLabel) {
      const groupId = groupLabel.dataset.groupId ?? '';
      if (!groupId) return;

      if (draggingGroupId) {
        if (groupId !== draggingGroupId) {
          const rect = groupLabel.getBoundingClientRect();
          dropPlacement = clientY > rect.top + rect.height / 2 ? 'after' : 'before';
          dropTargetGroupId = groupId;
        }
      } else if (draggingConnId) {
        dropTargetGroupId = groupId;
      }
    }
  }

  function handlePointerMove(e: MouseEvent) {
    if (!dragCandidate && !pointerDragging) return;

    if (!pointerDragging && dragCandidate) {
      const movedX = Math.abs(e.clientX - dragCandidate.startX);
      const movedY = Math.abs(e.clientY - dragCandidate.startY);
      if (Math.max(movedX, movedY) < 4) return;

      pointerDragging = true;
      suppressNextClick = true;
      if (dragCandidate.kind === 'conn') {
        draggingConnId = dragCandidate.id;
      } else {
        draggingGroupId = dragCandidate.id;
      }
      document.body.style.cursor = 'grabbing';
    }

    if (!pointerDragging) return;
    updatePointerDropTarget(e.clientX, e.clientY);
  }

  function finishPointerDrag() {
    dragCandidate = null;
    pointerDragging = false;
    draggingConnId = '';
    draggingGroupId = '';
    clearDropTargets();
    document.body.style.cursor = '';
  }

  function handlePointerUp(e: MouseEvent) {
    if (!dragCandidate && !pointerDragging) return;

    if (!pointerDragging) {
      dragCandidate = null;
      return;
    }

    const target = document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null;
    const connLabel = target?.closest('.conn-label') as HTMLElement | null;
    const groupLabel = target?.closest('.group-label') as HTMLElement | null;
    const navContent = target?.closest('.nav-content') as HTMLElement | null;

    if (draggingConnId) {
      if (connLabel) {
        const targetConnId = connLabel.dataset.connId ?? '';
        const targetGroupId = connLabel.dataset.groupId || undefined;
        if (targetConnId && targetConnId !== draggingConnId) {
          moveConnectionInList(draggingConnId, targetConnId, dropPlacement, targetGroupId);
          if (targetGroupId) {
            expandedGroups[targetGroupId] = true;
            expandedGroups = { ...expandedGroups };
            activeServerGroupId.set(targetGroupId);
          } else {
            activeServerGroupId.set('');
          }
        }
      } else if (groupLabel) {
        const targetGroupId = groupLabel.dataset.groupId ?? '';
        if (targetGroupId) {
          addConnectionToGroup(draggingConnId, targetGroupId);
          expandedGroups[targetGroupId] = true;
          expandedGroups = { ...expandedGroups };
          activeServerGroupId.set(targetGroupId);
        }
      } else if (navContent) {
        moveConnectionInList(draggingConnId, null, 'end');
        activeServerGroupId.set('');
      }
    } else if (draggingGroupId && groupLabel) {
      const targetGroupId = groupLabel.dataset.groupId ?? '';
      if (targetGroupId && targetGroupId !== draggingGroupId) {
        moveServerGroup(draggingGroupId, targetGroupId, dropPlacement);
      }
    }

    finishPointerDrag();
  }

  function maybeSuppressClick() {
    if (!suppressNextClick) return false;
    suppressNextClick = false;
    return true;
  }

  // Context menu state
  let contextMenu:
    | { kind: 'table'; x: number; y: number; tableName: string; connId: string; schemaName?: string }
    | { kind: 'database'; x: number; y: number; connId: string }
    | { kind: 'dropConfirm'; x: number; y: number; tableName: string; connId: string; schemaName?: string }
    | null = null;

  function clampMenuPosition(x: number, y: number, kind: 'table' | 'database' | 'dropConfirm') {
    const pad = 8;
    const estimatedWidth = 220;
    const estimatedHeight = kind === 'database' ? 285 : kind === 'dropConfirm' ? 120 : 165;
    const maxX = Math.max(pad, window.innerWidth - estimatedWidth - pad);
    const maxY = Math.max(pad, window.innerHeight - estimatedHeight - pad);
    return {
      x: Math.max(pad, Math.min(x, maxX)),
      y: Math.max(pad, Math.min(y, maxY)),
    };
  }

  function openTableContextMenu(e: MouseEvent, connId: string, tableName: string, schemaName?: string) {
    e.preventDefault();
    e.stopPropagation();
    const pos = clampMenuPosition(e.clientX, e.clientY, 'table');
    contextMenu = { kind: 'table', x: pos.x, y: pos.y, tableName, connId, schemaName };
  }

  function openDatabaseContextMenu(e: MouseEvent, connId: string) {
    e.preventDefault();
    e.stopPropagation();
    const pos = clampMenuPosition(e.clientX, e.clientY, 'database');
    contextMenu = { kind: 'database', x: pos.x, y: pos.y, connId };
  }

  async function handleContextAction(action: 'view' | 'copy' | 'select' | 'backup' | 'dropTable' | 'query' | 'import' | 'refresh' | 'relationships' | 'connections' | 'test' | 'cancelTest' | 'delete') {
    if (!contextMenu) return;
    const menu = contextMenu;
    contextMenu = null;

    try {
      if (menu.kind === 'database') {
        if (action === 'query') {
          openQueryTabForConnection(menu.connId);
          return;
        }
        if (action === 'refresh') {
          statusMessage.set('Refreshing schema…');
          await refreshSchema(menu.connId);
          statusMessage.set('Schema refreshed');
          return;
        }
        if (action === 'relationships') {
          await showRelationshipDiagramForConnection(menu.connId);
          return;
        }
        if (action === 'connections') {
          showDatabaseConnectionsForConnection(menu.connId);
          return;
        }
        if (action === 'import') {
          importDialogConnId.set(menu.connId);
          showImportDialog.set(true);
          return;
        }
        if (action === 'test') {
          const conn = get(activeConnections).find(c => c.config.id === menu.connId);
          if (conn) await testConn(conn);
          return;
        }
        if (action === 'cancelTest') {
          await cancelTest();
          return;
        }
        if (action === 'delete') {
          await disconnectConn(menu.connId);
          return;
        }
        return;
      }

      if (menu.kind !== 'table') return;

      const { connId, tableName, schemaName } = menu;
      if (action === 'view') openTableQuery(connId, tableName, schemaName);
      else if (action === 'copy') copyName(qualifyTable(connId, tableName, schemaName));
      else if (action === 'select') openTableQuery(connId, tableName, schemaName);
      else if (action === 'backup') {
        await BackupTable(connId, tableName, schemaName ?? '');
        statusMessage.set(`Backed up ${qualifyTable(connId, tableName, schemaName)}`);
      } else if (action === 'dropTable') {
        const pos = clampMenuPosition(menu.x, menu.y, 'dropConfirm');
        contextMenu = { kind: 'dropConfirm', x: pos.x, y: pos.y, connId, tableName, schemaName };
      }
    } catch (e: any) {
      statusMessage.set(String(e));
    }
  }

  async function confirmDropTable() {
    if (!contextMenu || contextMenu.kind !== 'dropConfirm') return;
    const { connId, tableName, schemaName } = contextMenu;
    contextMenu = null;

    try {
      await DropTable(connId, tableName, schemaName ?? '');
      statusMessage.set(`Dropped ${qualifyTable(connId, tableName, schemaName)}`);
      await refreshSchema(connId);
    } catch (e: any) {
      statusMessage.set(String(e));
    }
  }

  function closeContextMenu() {
    closeFloatingUi();
  }

  function handleWindowClick(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (target.closest('.nav-header') || target.closest('.header-menu') || target.closest('.context-menu')) {
      return;
    }
    closeFloatingUi();
  }

  // ── Schema refresh ─────────────────────────────────────────────────────────
  // Triggered by the context menu "Refresh" action OR by SqlEditor after a DDL
  // statement completes (via the schemaRefreshSignal store).
  async function refreshSchema(connId: string) {
    await refreshConnectionSchema(connId);
  }

  // Watch the shared signal so any component can request a schema refresh
  // without needing a direct reference to this component.
  let _lastRefreshTs = 0;
  $: if ($schemaRefreshSignal && $schemaRefreshSignal.ts !== _lastRefreshTs) {
    _lastRefreshTs = $schemaRefreshSignal.ts;
    refreshSchema($schemaRefreshSignal.connId);
  }
</script>

<svelte:window on:mousemove={handlePointerMove} on:mouseup={handlePointerUp} on:click={handleWindowClick} />

<aside class="navigator">
  <div class="nav-header">
    <div class="nav-title-row">
      <span class="nav-title">Connections</span>
      <button class="icon-btn header-icon" on:click={() => { addMenuOpen = !addMenuOpen; settingsOpen = false; }} title="Add">+</button>
      {#if addMenuOpen}
        <div class="header-menu add-menu" role="menu" tabindex="-1">
          <button role="menuitem" on:click={openNewConnection}>New Connection</button>
          <button role="menuitem" on:click={createServerGroup}>New Server Group</button>
        </div>
      {/if}
    </div>
    <button class="icon-btn header-icon" on:click={() => { settingsOpen = !settingsOpen; addMenuOpen = false; }} title="Settings">⚙</button>
    {#if settingsOpen}
      <div class="header-menu settings-menu" role="dialog" aria-label="Navigator settings" tabindex="-1">
        <div class="settings-row">
          <span>Font size</span>
          <div class="font-controls">
            <button class="icon-btn scale-btn" on:click={() => adjustFontScale(-10)} title="Decrease font size">−</button>
            <input
              type="number"
              min="50"
              max="250"
              step="10"
              bind:value={fontScaleInput}
              on:change={commitFontScale}
              on:keydown={e => e.key === 'Enter' && commitFontScale()}
              aria-label="Font size percentage"
            />
            <span class="percent-mark">%</span>
            <button class="icon-btn scale-btn" on:click={() => adjustFontScale(10)} title="Increase font size">+</button>
          </div>
        </div>
      </div>
    {/if}
  </div>

  <div class="nav-filter">
    <input
      class="nav-filter-input"
      type="search"
      value={tableFilter}
      placeholder="Filter tables or columns"
      autocomplete="off"
      autocapitalize="none"
      spellcheck="false"
      aria-label="Filter tables or columns"
      on:input={updateTableFilter}
    />
    {#if tableFilter}
      <button class="nav-filter-clear" type="button" on:click={clearTableFilter} aria-label="Clear table filter">Clear</button>
    {/if}
  </div>

  <div
    class="nav-content"
    role="tree"
    tabindex="-1"
  >
    {#each navItems as item (item.kind === 'group' ? `group-${item.group.id}` : `conn-${item.conn.config.id}-${item.groupId ?? 'root'}`)}
      {#if item.kind === 'group'}
        <div class="group-node">
          <div
            class="group-label"
            data-group-id={item.group.id}
            class:active={$activeServerGroupId === item.group.id}
            class:drop-before={dropTargetGroupId === item.group.id && dropPlacement === 'before'}
            class:drop-after={dropTargetGroupId === item.group.id && dropPlacement === 'after'}
            class:dragging={draggingGroupId === item.group.id}
            on:mousedown={e => beginGroupPointerDrag(e, item.group.id)}
            on:click={() => {
              if (maybeSuppressClick()) return;
              toggleGroup(item.group.id);
            }}
            role="treeitem"
            aria-selected={$activeServerGroupId === item.group.id}
            aria-expanded={hasTableFilter || !!expandedGroups[item.group.id]}
            tabindex="0"
            on:keydown={e => e.key === 'Enter' && toggleGroup(item.group.id)}
          >
            <span class="chevron">{hasTableFilter || expandedGroups[item.group.id] ? '▾' : '▸'}</span>
            <span class="table-icon">▣</span>
            <span class="node-name">{item.group.title}</span>
            <span class="count">({groupConnectionCount(item.group)})</span>
          </div>
        </div>
      {:else}
        {@const conn = item.conn}
      <div class="conn-node">
        <div
          class="conn-label"
          data-conn-id={conn.config.id}
          data-group-id={item.groupId ?? ''}
          class:selected={$selectedConnId === conn.config.id}
          class:grouped={!!item.groupId}
          class:drop-before={dropTargetConnId === conn.config.id && dropPlacement === 'before'}
          class:drop-after={dropTargetConnId === conn.config.id && dropPlacement === 'after'}
          class:dragging={draggingConnId === conn.config.id}
          on:mousedown={e => beginConnPointerDrag(e, conn.config.id)}
          on:click={() => {
            if (maybeSuppressClick()) return;
            toggleConn(conn.config.id, conn);
          }}
          on:contextmenu={e => openDatabaseContextMenu(e, conn.config.id)}
          role="treeitem" aria-selected={false}
          aria-expanded={hasTableFilter || !!expanded[conn.config.id]}
          tabindex="0"
          on:keydown={e => e.key === 'Enter' && toggleConn(conn.config.id, conn)}
        >
          <span class="chevron">{hasTableFilter || expanded[conn.config.id] ? '▾' : '▸'}</span>
          <span class="conn-icon">🔌</span>
          <span
            class="conn-color-swatch"
            class:active={!!conn.config.tabColor}
            style="background:{getConnectionColorSwatch(conn.config.tabColor)}"
            title={conn.config.tabColor ? `Tab colour: ${conn.config.tabColor}` : 'No tab colour set'}
            aria-label="Connection colour"
          ></span>
          <span class="conn-name">
            <span class="node-name">{conn.config.name}</span>
            {#if formatBytes(conn.schema?.sizeBytes)}
              <span class="size-label">{formatBytes(conn.schema?.sizeBytes)}</span>
            {/if}
          </span>
          <span class="driver-badge">{conn.config.driver}</span>
          <div class="conn-actions">
            <button class="icon-btn" on:click|stopPropagation={() => editConn(conn)} title="Edit">✏️</button>
            <button class="icon-btn" on:click|stopPropagation={() => disconnectConn(conn.config.id)} title="Disconnect">✕</button>
          </div>
        </div>

        {#if hasTableFilter || expanded[conn.config.id]}
          <div class="conn-children">
            {#if conn.schemaLoading}
              <div class="nav-info">Loading schema…</div>
            {:else if conn.schemaError}
              <div class="nav-error">{conn.schemaError}</div>
            {:else if conn.schema}
              {#if conn.schema.schemas?.length}
                <!-- Postgres: schema-grouped hierarchy -->
                {#each conn.schema.schemas as pgSchema}
                  {@const schemaKey = `${conn.config.id}-schema-${pgSchema.name}`}
                  {@const tablesKey = `${conn.config.id}-${pgSchema.name}-tables`}
                  {@const filteredPgTables = filterTables(pgSchema.tables)}
                  {#if !hasTableFilter || filteredPgTables.length > 0}
                  <div class="schema-section">
                    <div
                      class="section-label schema-node"
                      on:click={() => toggleTable(schemaKey)}
                      role="treeitem" aria-selected={false}
                      aria-expanded={hasTableFilter || !!expandedTables[schemaKey]}
                      tabindex="0"
                      on:keydown={e => e.key === 'Enter' && toggleTable(schemaKey)}
                    >
                      <span class="chevron">{hasTableFilter || expandedTables[schemaKey] ? '▾' : '▸'}</span>
                      <span class="table-icon">🗂</span>
                      <span class="node-name">{pgSchema.name}</span>
                      {#if formatBytes(pgSchema.sizeBytes)}
                        <span class="size-label">{formatBytes(pgSchema.sizeBytes)}</span>
                      {/if}
                    </div>
                    {#if hasTableFilter || expandedTables[schemaKey]}
                      <div class="conn-children">
                        <!-- Tables -->
                        <div class="schema-section">
                          <div
                            class="section-label"
                            on:click={() => toggleTable(tablesKey)}
                            role="treeitem" aria-selected={false}
                            aria-expanded={hasTableFilter || !!expandedTables[tablesKey]}
                            tabindex="0"
                            on:keydown={e => e.key === 'Enter' && toggleTable(tablesKey)}
                          >
                            <span class="chevron">{hasTableFilter || expandedTables[tablesKey] ? '▾' : '▸'}</span>
                            Tables <span class="count">({filteredPgTables.length})</span>
                          </div>
                          {#if hasTableFilter || expandedTables[tablesKey]}
                            {#each filteredPgTables as table}
                              <div
                                class="table-label"
                                on:click={() => toggleTable(`${conn.config.id}-${pgSchema.name}-t-${table.name}`)}
                                on:contextmenu={e => openTableContextMenu(e, conn.config.id, table.name, pgSchema.name)}
                                role="treeitem" aria-selected={false}
                                tabindex="0"
                                on:keydown={e => e.key === 'Enter' && toggleTable(`${conn.config.id}-${pgSchema.name}-t-${table.name}`)}
                              >
                                <span class="chevron">{expandedTables[`${conn.config.id}-${pgSchema.name}-t-${table.name}`] ? '▾' : '▸'}</span>
                                <span class="table-icon">📋</span>
                                <span class="node-name">{table.name}</span>
                                {#if formatBytes(table.sizeBytes)}
                                  <span class="size-label">{formatBytes(table.sizeBytes)}</span>
                                {/if}
                              </div>
                              {#if expandedTables[`${conn.config.id}-${pgSchema.name}-t-${table.name}`]}
                                <div class="col-list">
                                  {#each table.columns ?? [] as col}
                                    <div class="col-row">
                                      <span class="col-name">{col.name}</span>
                                      {#if col.key === 'PRI'}
                                        <span class="col-key" title={col.key}>🔑</span>
                                      {/if}
                                      <span class="col-type">{col.type}</span>
                                    </div>
                                  {/each}
                                </div>
                              {/if}
                            {/each}
                          {/if}
                        </div>
                        <!-- Views -->
                        {#if !hasTableFilter && pgSchema.views?.length > 0}
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
                        {#if !hasTableFilter && pgSchema.indexes?.length > 0}
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
                  {/if}
                {/each}
              {:else}
              <!-- MySQL / SQLite: flat hierarchy -->
              <!-- Tables -->
              {@const tablesKey = `${conn.config.id}-tables`}
              {@const filteredTables = filterTables(conn.schema.tables)}
              {#if !hasTableFilter || filteredTables.length > 0}
              <div class="schema-section">
                <div
                  class="section-label"
                  on:click={() => toggleTable(tablesKey)}
                  role="treeitem" aria-selected={false}
                  aria-expanded={hasTableFilter || !!expandedTables[tablesKey]}
                  tabindex="0"
                  on:keydown={e => e.key === 'Enter' && toggleTable(tablesKey)}
                >
                  <span class="chevron">{hasTableFilter || expandedTables[tablesKey] ? '▾' : '▸'}</span>
                  Tables <span class="count">({filteredTables.length})</span>
                </div>
                {#if hasTableFilter || expandedTables[tablesKey]}
                  {#each filteredTables as table}
                    <div
                      class="table-label"
                      on:click={() => toggleTable(`${conn.config.id}-t-${table.name}`)}
                      on:contextmenu={e => openTableContextMenu(e, conn.config.id, table.name)}
                      role="treeitem" aria-selected={false}
                      tabindex="0"
                      on:keydown={e => e.key === 'Enter' && toggleTable(`${conn.config.id}-t-${table.name}`)}
                    >
                      <span class="chevron">{expandedTables[`${conn.config.id}-t-${table.name}`] ? '▾' : '▸'}</span>
                      <span class="table-icon">📋</span>
                      <span class="node-name">{table.name}</span>
                      {#if formatBytes(table.sizeBytes)}
                        <span class="size-label">{formatBytes(table.sizeBytes)}</span>
                      {/if}
                    </div>
                    {#if expandedTables[`${conn.config.id}-t-${table.name}`]}
                      <div class="col-list">
                        {#each table.columns ?? [] as col}
                          <div class="col-row">
                            <span class="col-name">{col.name}</span>
                            {#if col.key === 'PRI'}
                              <span class="col-key" title={col.key}>🔑</span>
                            {/if}
                            <span class="col-type">{col.type}</span>
                          </div>
                        {/each}
                      </div>
                    {/if}
                  {/each}
                {/if}
              </div>
              {/if}

              <!-- Views -->
              {#if !hasTableFilter && conn.schema.views?.length > 0}
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
              {#if !hasTableFilter && conn.schema.indexes?.length > 0}
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
      {/if}
    {/each}

    {#if $activeConnections.length === 0}
      <div class="empty-nav">
        <p>No connections.</p>
        <button class="btn-link" on:click={openNewConnection}>+ New Connection</button>
      </div>
    {/if}
  </div>
</aside>

{#if contextMenu}
  <div
    class="context-menu"
    style="left:{contextMenu.x}px; top:{contextMenu.y}px"
    role="menu"
    tabindex="0"
    on:click|stopPropagation
    on:keydown|stopPropagation={() => {}}
  >
    {#if contextMenu.kind === 'table'}
      <button role="menuitem" on:click={() => handleContextAction('view')}>
        View Data (SELECT * LIMIT 100)
      </button>
      <button role="menuitem" on:click={() => handleContextAction('copy')}>
        Copy Name
      </button>
      <button role="menuitem" on:click={() => handleContextAction('backup')}>
        Backup Table...
      </button>
      <div class="context-separator"></div>
      <button role="menuitem" class="danger" on:click={() => handleContextAction('dropTable')}>
        Drop Table...
      </button>
    {:else if contextMenu.kind === 'database'}
      <button role="menuitem" on:click={() => handleContextAction('query')}>
        Query
      </button>
      <div class="context-separator"></div>
      {#if testingConnId === contextMenu.connId}
        <button role="menuitem" on:click={() => handleContextAction('cancelTest')}>
          Cancel Test
        </button>
      {:else}
        <button role="menuitem" on:click={() => handleContextAction('test')}>
          Test Connection
        </button>
      {/if}
      <div class="context-separator"></div>
      <button role="menuitem" on:click={() => handleContextAction('refresh')}>
        Refresh Schema
      </button>
      <button role="menuitem" on:click={() => handleContextAction('relationships')}>
        Show Relationships
      </button>
      <button role="menuitem" on:click={() => handleContextAction('connections')}>
        Show Connections
      </button>
      <button role="menuitem" on:click={() => handleContextAction('import')}>
        Import...
      </button>
      <div class="context-separator"></div>
      <button role="menuitem" class="danger" on:click={() => handleContextAction('delete')}>
        Remove Connection
      </button>
    {:else if contextMenu.kind === 'dropConfirm'}
      <div class="context-title">Drop {contextMenu.schemaName ? `${contextMenu.schemaName}.${contextMenu.tableName}` : contextMenu.tableName}?</div>
      <button role="menuitem" class="danger" on:click={confirmDropTable}>
        Yes, drop table
      </button>
      <button role="menuitem" on:click={closeContextMenu}>
        No, cancel
      </button>
    {/if}
  </div>
{/if}

{#if showServerGroupDialog}
  <div class="modal-overlay" role="dialog" aria-modal="true" aria-label="New Server Group">
    <div class="modal">
      <div class="modal-header">
        <h2>New Server Group</h2>
        <button class="close-btn" on:click={closeServerGroupDialog} aria-label="Close">✕</button>
      </div>

      <div class="modal-body">
        <div class="form-row">
          <label>Group Title
            <!-- svelte-ignore a11y-autofocus -->
            <input
              type="text"
              bind:value={serverGroupTitle}
              placeholder="Production"
              autocomplete="off"
              autocapitalize="none"
              spellcheck="false"
              autofocus
              on:keydown={e => e.key === 'Enter' && saveServerGroup()}
            />
          </label>
        </div>
        {#if serverGroupError}
          <p class="error">{serverGroupError}</p>
        {/if}
      </div>

      <div class="modal-footer">
        <button class="btn-secondary" on:click={closeServerGroupDialog}>Cancel</button>
        <button class="btn-primary" on:click={saveServerGroup}>Create Group</button>
      </div>
    </div>
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
    position: relative;
    display: flex; align-items: center; justify-content: space-between;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    font-size: calc(11px * var(--app-font-scale)); font-weight: 600; text-transform: uppercase; color: var(--text-muted);
  }
  .nav-title-row { display: inline-flex; align-items: center; gap: 6px; min-width: 0; }
  .nav-title { letter-spacing: 0.05em; }
  .header-icon {
    width: 22px;
    height: 22px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 4px;
  }
  .header-icon:hover { background: var(--bg-hover); }
  .header-menu {
    position: absolute;
    z-index: 310;
    top: calc(100% + 4px);
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 4px;
    box-shadow: 0 4px 16px rgba(0,0,0,0.4);
    text-transform: none;
    font-weight: 400;
    color: var(--text);
  }
  .add-menu { left: 12px; min-width: 170px; overflow: hidden; }
  .settings-menu { right: 8px; width: 220px; padding: 10px; }
  .header-menu button {
    display: block;
    width: 100%;
    padding: 8px 12px;
    border: 0;
    background: transparent;
    color: var(--text);
    text-align: left;
    cursor: pointer;
    font-size: calc(12px * var(--app-font-scale));
  }
  .header-menu button:hover { background: var(--bg-hover); }
  .settings-row {
    display: flex;
    flex-direction: column;
    gap: 8px;
    font-size: calc(12px * var(--app-font-scale));
    color: var(--text-muted);
  }
  .font-controls {
    display: grid;
    grid-template-columns: 26px 1fr auto 26px;
    align-items: center;
    gap: 6px;
  }
  .font-controls input {
    min-width: 0;
    height: 26px;
    box-sizing: border-box;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-input);
    color: var(--text);
    padding: 3px 6px;
    font-size: calc(12px * var(--app-font-scale));
  }
  .percent-mark { color: var(--text-muted); }
  .scale-btn {
    width: 26px;
    height: 26px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-input);
    text-align: center;
  }
  .nav-content { flex: 1; overflow-y: auto; padding: 4px 0 40px; }
  .nav-filter {
    position: relative;
    padding: 8px;
    border-bottom: 1px solid var(--border);
  }
  .nav-filter-input {
    width: 100%;
    height: 28px;
    padding: 5px 50px 5px 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-input);
    color: var(--text);
    box-sizing: border-box;
    font-size: calc(12px * var(--app-font-scale));
  }
  .nav-filter-input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .nav-filter-clear {
    position: absolute;
    top: 50%;
    right: 14px;
    transform: translateY(-50%);
    border: 0;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    font-size: calc(11px * var(--app-font-scale));
    padding: 2px 4px;
  }
  .nav-filter-clear:hover { color: var(--text); }

  .group-label {
    display: flex; align-items: center; gap: 4px;
    padding: 5px 8px; cursor: grab; user-select: none;
    font-size: calc(13px * var(--app-font-scale)); color: var(--text);
  }
  .group-label:active { cursor: grabbing; }
  .group-label.dragging { opacity: 0.6; }
  .group-label:hover,
  .group-label.active { background: var(--bg-hover); }
  .group-label.drop-before { box-shadow: inset 0 2px 0 0 var(--accent); }
  .group-label.drop-after { box-shadow: inset 0 -2px 0 0 var(--accent); }
  .group-label:focus { outline: 1px solid var(--accent); }

  .conn-label {
    display: flex; align-items: center; gap: 4px;
    padding: 5px 8px; cursor: grab; user-select: none;
    font-size: calc(13px * var(--app-font-scale)); color: var(--text);
  }
  .conn-label:active { cursor: grabbing; }
  .conn-label.dragging { opacity: 0.6; }
  .conn-label.grouped { padding-left: 24px; }
  .conn-label:hover { background: var(--bg-hover); }
  .conn-label.selected { background: var(--bg-selected); }
  .conn-label.drop-before { box-shadow: inset 0 2px 0 0 var(--accent); }
  .conn-label.drop-after { box-shadow: inset 0 -2px 0 0 var(--accent); }
  .conn-label:focus { outline: 1px solid var(--accent); }
  .conn-name {
    display: inline-flex; align-items: baseline; gap: 6px;
    font-weight: 500; flex: 1; min-width: 0; overflow: hidden; white-space: nowrap;
  }
  .node-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; }
  .size-label {
    flex: 0 0 auto;
    color: var(--text-muted);
    font-size: calc(11px * var(--app-font-scale));
    font-weight: 400;
    opacity: 0.72;
  }
  .driver-badge {
    font-size: calc(10px * var(--app-font-scale)); padding: 1px 5px; border-radius: 3px;
    background: var(--bg-badge); color: var(--text-muted);
  }
  .conn-actions { display: none; gap: 2px; align-items: center; }
  .conn-label:hover .conn-actions { display: flex; }
  .conn-icon, .table-icon { flex-shrink: 0; }
  .conn-color-swatch {
    width: 10px;
    height: 10px;
    border-radius: 2px;
    border: 1px solid var(--border);
    background: transparent;
    flex-shrink: 0;
  }
  .conn-color-swatch.active {
    border-color: rgba(255, 255, 255, 0.35);
  }

  .conn-children { padding-left: 16px; }
  .conn-children > .schema-section { margin-left: 16px; }
  .schema-section { margin: 2px 0; }
  .section-label {
    display: flex; align-items: center; gap: 4px;
    padding: 3px 8px; cursor: pointer; user-select: none;
    font-size: calc(12px * var(--app-font-scale)); color: var(--text-muted); font-weight: 500;
  }
  .section-label:hover { color: var(--text); }
  .section-label.schema-node { color: var(--text); font-size: calc(13px * var(--app-font-scale)); }
  .count { font-weight: 400; opacity: 0.7; }

  .table-label {
    display: flex; align-items: center; gap: 4px;
    margin-left: 16px;
    padding: 3px 8px; cursor: pointer; user-select: none;
    font-size: calc(12px * var(--app-font-scale)); color: var(--text);
  }
  .table-label:hover { background: var(--bg-hover); }
  .table-label.leaf { padding-left: 24px; }

  .col-list { margin-left: 16px; padding-left: 24px; }
  .col-row {
    display: flex; align-items: center; gap: 6px;
    padding: 2px 8px; font-size: calc(11px * var(--app-font-scale)); color: var(--text-muted);
  }
  .col-key { width: 14px; flex-shrink: 0; }
  .col-name { flex: 1; }
  .col-type { opacity: 0.6; font-style: italic; }

  .chevron { opacity: 0.5; font-size: calc(10px * var(--app-font-scale)); width: 10px; flex-shrink: 0; }
  .icon-btn {
    background: none; border: none; cursor: pointer;
    color: var(--text-muted); font-size: calc(12px * var(--app-font-scale)); padding: 2px 4px;
    line-height: 1;
  }
  .icon-btn:hover { color: var(--text); }

  .nav-info { padding: 6px 12px; font-size: calc(12px * var(--app-font-scale)); color: var(--text-muted); }
  .nav-error { padding: 6px 12px; font-size: calc(12px * var(--app-font-scale)); color: var(--error); }

  .empty-nav { padding: 20px 16px; text-align: center; }
  .empty-nav p { font-size: calc(12px * var(--app-font-scale)); color: var(--text-muted); margin: 0 0 8px; }
  .btn-link { background: none; border: none; color: var(--accent); cursor: pointer; font-size: calc(12px * var(--app-font-scale)); }
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
    color: var(--text); font-size: calc(13px * var(--app-font-scale)); cursor: pointer;
  }
  .context-menu button:hover { background: var(--bg-hover); }
  .context-menu button:disabled { opacity: 0.5; cursor: default; }
  .context-menu button:disabled:hover { background: none; }
  .context-menu button.danger { color: var(--error); }
  .context-menu button.danger:hover { background: rgba(248, 113, 113, 0.1); }
  .context-separator {
    height: 1px; background: var(--border); margin: 3px 0;
  }
  .context-title {
    font-size: calc(12px * var(--app-font-scale));
    color: var(--text-muted);
    padding: 8px 16px;
    border-bottom: 1px solid var(--border);
  }

  .modal-overlay {
    position: fixed; inset: 0;
    background: rgba(0,0,0,0.6);
    display: flex; align-items: center; justify-content: center;
    z-index: 320;
  }
  .modal {
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    width: 420px;
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
  label { display: flex; flex-direction: column; gap: 4px; font-size: calc(12px * var(--app-font-scale)); color: var(--text-muted); }
  input {
    background: var(--bg-input); border: 1px solid var(--border);
    color: var(--text); padding: 7px 10px; border-radius: 4px;
    font-size: calc(13px * var(--app-font-scale)); width: 100%; box-sizing: border-box;
  }
  input:focus { outline: none; border-color: var(--accent); }
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
  .btn-secondary { background: var(--bg-input); color: var(--text); border-color: var(--border); }
  .btn-secondary:hover { border-color: var(--accent); }
  .error { color: var(--error); font-size: calc(12px * var(--app-font-scale)); margin: 0; }
</style>
