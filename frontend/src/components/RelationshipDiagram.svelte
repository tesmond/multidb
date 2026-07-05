<script lang="ts">
  import { get } from 'svelte/store';
  import {
    activeConnections,
    activeTabId,
    isRelationshipDiagramTab,
    selectedConnId,
    tabs,
  } from '../stores/appStore';
  import {
    buildRelationshipDiagramQuerySql,
    buildRelationshipDiagramGraph,
    createDefaultRelationshipLayout,
    filterRelationshipDiagramGraph,
    getEffectiveSelectedEdgeId,
    getRelationshipLayoutSchemaHash,
    loadRelationshipLayout,
    mergeRelationshipDiagramLayout,
    projectRelationshipDiagramLayout,
    resetRelationshipLayout,
    saveRelationshipLayout,
    type DiagramGraph,
    type DiagramLayout,
    type DiagramRelationshipEdge,
    type DiagramTableNode,
  } from '../lib/relationshipDiagram';

  export let tabId: string;

  const HEADER_HEIGHT = 40;
  const ROW_HEIGHT = 24;

  let panX = 40;
  let panY = 32;
  let zoom = 1;
  let containerEl: HTMLDivElement;
  let tab = null as import('../stores/appStore').RelationshipDiagramTab | null;
  let connection = null as (typeof $activeConnections)[number] | null;
  let schema = null as (typeof $activeConnections)[number]['schema'] | null;
  let graph: DiagramGraph = { tables: [], edges: [] };
  let schemaHash = '';
  let tableLayout: DiagramLayout = {};
  let layoutKey = '';
  let lastSchemaHash = '';
  let hoveredEdgeId = '';
  let selectedEdgeId = '';
  let filterText = '';
  let dragTableId = '';
  let dragStartClientX = 0;
  let dragStartClientY = 0;
  let dragOriginX = 0;
  let dragOriginY = 0;
  let isPanning = false;
  let panStartClientX = 0;
  let panStartClientY = 0;
  let panOriginX = 0;
  let panOriginY = 0;

  $: {
    const current = $tabs.find((entry) => entry.id === tabId);
    tab = isRelationshipDiagramTab(current) ? current : null;
  }

  $: {
    const currentTab = tab;
    connection = currentTab
      ? $activeConnections.find((entry) => entry.config.id === currentTab.connId) ?? null
      : null;
  }

  $: schema = connection?.schema ?? null;
  $: graph = schema ? buildRelationshipDiagramGraph(schema) : { tables: [], edges: [] };
  $: schemaHash = schema ? getRelationshipLayoutSchemaHash(schema) : '';
  $: defaultLayout = createDefaultRelationshipLayout(graph);
  $: {
    const nextLayoutKey = tab && schemaHash
      ? `${tab.connId}:${schemaHash}:${graph.tables.map((table) => table.id).join('|')}`
      : '';
    if (nextLayoutKey !== layoutKey) {
      const previousLayout = tableLayout;
      const previousSchemaHash = lastSchemaHash;
      layoutKey = nextLayoutKey;
      const persisted = tab && schemaHash ? loadRelationshipLayout(tab.connId, schemaHash) : null;
      const carryForwardLayout =
        persisted ?? (previousSchemaHash && previousSchemaHash !== schemaHash ? previousLayout : previousLayout);
      tableLayout = mergeRelationshipDiagramLayout(graph, defaultLayout, carryForwardLayout);
      if (tab && schemaHash && previousLayout && Object.keys(previousLayout).length > 0) {
        saveRelationshipLayout(tab.connId, schemaHash, tableLayout);
      }
      lastSchemaHash = schemaHash;
      if (!graph.edges.find((edge) => edge.id === selectedEdgeId)) {
        selectedEdgeId = graph.edges[0]?.id ?? '';
      }
    }
  }

  $: visibleGraph = filterRelationshipDiagramGraph(graph, filterText);
  $: visibleTables = visibleGraph.tables;
  $: visibleEdges = visibleGraph.edges;

  $: effectiveSelectedEdgeId = getEffectiveSelectedEdgeId(visibleEdges, selectedEdgeId);
  $: selectedEdge = visibleEdges.find((edge) => edge.id === effectiveSelectedEdgeId) ?? null;
  $: activeEdgeId = hoveredEdgeId || effectiveSelectedEdgeId;
  $: highlightedColumnIds = new Set(
    (visibleEdges.find((edge) => edge.id === activeEdgeId)?.sourceColumnIds ?? []).concat(
      visibleEdges.find((edge) => edge.id === activeEdgeId)?.targetColumnIds ?? [],
    ),
  );
  $: visibleLayout = projectRelationshipDiagramLayout(visibleGraph, tableLayout);
  $: contentBounds = getContentBounds(visibleTables, visibleLayout);

  function zoomBy(factor: number) {
    const rect = containerEl?.getBoundingClientRect();
    if (!rect) {
      zoom = clampZoom(zoom * factor);
      return;
    }
    const pivotX = rect.width / 2;
    const pivotY = rect.height / 2;
    applyZoom(zoom * factor, pivotX, pivotY);
  }

  function applyZoom(nextZoom: number, pivotX: number, pivotY: number) {
    const clamped = clampZoom(nextZoom);
    const worldX = (pivotX - panX) / zoom;
    const worldY = (pivotY - panY) / zoom;
    zoom = clamped;
    panX = pivotX - worldX * zoom;
    panY = pivotY - worldY * zoom;
  }

  function clampZoom(value: number): number {
    return Math.max(0.5, Math.min(2.5, Number(value.toFixed(2))));
  }

  function handleWheel(event: WheelEvent) {
    event.preventDefault();
    const rect = containerEl?.getBoundingClientRect();
    if (!rect) return;
    const pivotX = event.clientX - rect.left;
    const pivotY = event.clientY - rect.top;
    const factor = event.deltaY < 0 ? 1.1 : 0.9;
    applyZoom(zoom * factor, pivotX, pivotY);
  }

  function beginPan(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (target.closest('.table-card') || target.closest('.diagram-toolbar') || target.closest('.relationship-inspector')) {
      return;
    }
    isPanning = true;
    panStartClientX = event.clientX;
    panStartClientY = event.clientY;
    panOriginX = panX;
    panOriginY = panY;
  }

  function beginTableDrag(event: MouseEvent, tableId: string) {
    event.stopPropagation();
    dragTableId = tableId;
    dragStartClientX = event.clientX;
    dragStartClientY = event.clientY;
    dragOriginX = tableLayout[tableId]?.x ?? 0;
    dragOriginY = tableLayout[tableId]?.y ?? 0;
  }

  function handlePointerMove(event: MouseEvent) {
    if (dragTableId) {
      const deltaX = (event.clientX - dragStartClientX) / zoom;
      const deltaY = (event.clientY - dragStartClientY) / zoom;
      tableLayout = {
        ...tableLayout,
        [dragTableId]: {
          x: Math.round(dragOriginX + deltaX),
          y: Math.round(dragOriginY + deltaY),
        },
      };
      return;
    }

    if (isPanning) {
      panX = panOriginX + (event.clientX - panStartClientX);
      panY = panOriginY + (event.clientY - panStartClientY);
    }
  }

  function handlePointerUp() {
    if (dragTableId && tab && schemaHash) {
      saveRelationshipLayout(tab.connId, schemaHash, tableLayout);
    }
    dragTableId = '';
    isPanning = false;
  }

  function handleResetLayout() {
    if (!tab || !schemaHash) return;
    resetRelationshipLayout(tab.connId, schemaHash);
    tableLayout = { ...defaultLayout };
  }

  function openTableQuery(table: DiagramTableNode) {
    if (!tab || !connection) return;
    const connId = tab.connId;
    const driver = connection.config.driver ?? 'postgres';
    const sql = buildRelationshipDiagramQuerySql(driver, table.schemaName, table.tableName);
    tabs.add(connId);
    const currentTabs = get(tabs);
    const nextTab = currentTabs[currentTabs.length - 1];
    tabs.updateTab(nextTab.id, {
      connId,
      title: table.tableName,
      sql,
    });
    selectedConnId.set(connId);
    activeTabId.set(nextTab.id);
  }

  function selectEdge(edgeId: string) {
    selectedEdgeId = edgeId;
  }

  function edgePath(edge: DiagramRelationshipEdge): string {
    const source = getColumnAnchor(edge.sourceTableId, edge.sourceColumnIds[0], true);
    const target = getColumnAnchor(edge.targetTableId, edge.targetColumnIds[0], false);
    if (!source || !target) return '';

    if (edge.isSelfReferential) {
      const loopX = source.x + 90;
      return `M ${source.x} ${source.y} C ${loopX} ${source.y}, ${loopX} ${target.y}, ${target.x} ${target.y}`;
    }

    const midX = Math.round((source.x + target.x) / 2);
    return `M ${source.x} ${source.y} L ${midX} ${source.y} L ${midX} ${target.y} L ${target.x} ${target.y}`;
  }

  function getColumnAnchor(tableId: string, columnId: string, sourceSide: boolean) {
    const table = graph.tables.find((entry) => entry.id === tableId);
    const position = visibleLayout[tableId] ?? tableLayout[tableId];
    if (!table || !position) return null;
    const columnIndex = table.columns.findIndex((column) => column.id === columnId);
    const rowIndex = columnIndex >= 0 ? columnIndex : 0;
    const x = sourceSide ? position.x + table.width : position.x;
    const y = position.y + HEADER_HEIGHT + rowIndex * ROW_HEIGHT + ROW_HEIGHT / 2;
    return { x, y };
  }

  function getContentBounds(tables: DiagramTableNode[], layout: DiagramLayout) {
    const maxX = tables.reduce((value, table) => Math.max(value, (layout[table.id]?.x ?? 0) + table.width), 0);
    const maxY = tables.reduce((value, table) => Math.max(value, (layout[table.id]?.y ?? 0) + table.height), 0);
    return {
      width: Math.max(900, maxX + 160),
      height: Math.max(640, maxY + 160),
    };
  }
</script>

<svelte:window on:mousemove={handlePointerMove} on:mouseup={handlePointerUp} />

{#if !tab || !connection}
  <div class="diagram-empty">Relationship viewer unavailable.</div>
{:else}
  <div class="diagram-shell">
    <div class="diagram-toolbar">
      <div class="toolbar-left">
        <span class="toolbar-title">Relationship Viewer</span>
        <span class="toolbar-meta">{connection.config.name}</span>
        <input
          class="diagram-filter"
          type="search"
          bind:value={filterText}
          placeholder="Filter tables or columns"
          aria-label="Filter relationship diagram"
        />
      </div>
      <div class="toolbar-actions">
        <button type="button" on:click={() => zoomBy(0.9)} aria-label="Zoom out">−</button>
        <span class="zoom-label">{Math.round(zoom * 100)}%</span>
        <button type="button" on:click={() => zoomBy(1.1)} aria-label="Zoom in">+</button>
        <button type="button" on:click={handleResetLayout}>Reset Layout</button>
      </div>
    </div>

    <div class="diagram-body">
      <div
        class="diagram-canvas"
        bind:this={containerEl}
        role="presentation"
        on:mousedown={beginPan}
        on:wheel={handleWheel}
      >
        <div
          class="diagram-stage"
          style={`width:${contentBounds.width}px;height:${contentBounds.height}px;transform: translate(${panX}px, ${panY}px) scale(${zoom});`}
        >
          <svg class="diagram-edges" viewBox={`0 0 ${contentBounds.width} ${contentBounds.height}`} preserveAspectRatio="none">
            <defs>
              <marker id="diagram-arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse">
                <path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"></path>
              </marker>
            </defs>
            {#each visibleEdges as edge (edge.id)}
              <path
                class="diagram-edge"
                class:hovered={hoveredEdgeId === edge.id}
                class:selected={selectedEdgeId === edge.id}
                d={edgePath(edge)}
                marker-end="url(#diagram-arrow)"
                role="button"
                tabindex="0"
                aria-label={`Relationship ${edge.constraintName}`}
                on:mouseenter={() => (hoveredEdgeId = edge.id)}
                on:mouseleave={() => (hoveredEdgeId = '')}
                on:click={() => selectEdge(edge.id)}
                on:keydown={(event) => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    selectEdge(edge.id);
                  }
                }}
              ></path>
            {/each}
          </svg>

          {#each visibleTables as table (table.id)}
            {@const position = visibleLayout[table.id] ?? { x: 0, y: 0 }}
            <article
              class="table-card"
              style={`left:${position.x}px;top:${position.y}px;width:${table.width}px;`}
            >
              <div class="table-card-header" role="presentation" on:mousedown={(event) => beginTableDrag(event, table.id)}>
                <div>
                  <h3>{table.title}</h3>
                  <p>{table.columns.length} columns</p>
                </div>
                <button type="button" class="open-query-btn" aria-label={`Open query for ${table.title}`} on:click={() => openTableQuery(table)}>
                  Open query
                </button>
              </div>

              <div class="table-columns">
                {#each table.columns as column (column.id)}
                  <div class="column-row" class:highlighted={highlightedColumnIds.has(column.id)}>
                    <span class="column-name">{column.name}</span>
                    <span class="column-badges">
                      {#if column.isPrimaryKey}<span class="badge pk">PK</span>{/if}
                      {#if column.isForeignKey}<span class="badge fk">FK</span>{/if}
                    </span>
                  </div>
                {/each}
              </div>
            </article>
          {/each}
        </div>

        {#if graph.edges.length === 0}
          <div class="diagram-notice">No foreign-key relationships found.</div>
        {:else if visibleTables.length === 0}
          <div class="diagram-notice">No tables match the current filter.</div>
        {/if}
      </div>

      <aside class="relationship-inspector">
        <h3>Relationship details</h3>
        {#if selectedEdge}
          <div class="inspector-card">
            <div class="inspector-name">{selectedEdge.constraintName}</div>
            <div class="inspector-row">
              <span>From</span>
              <span>{selectedEdge.sourceTableId}</span>
            </div>
            <div class="inspector-row">
              <span>To</span>
              <span>{selectedEdge.targetTableId}</span>
            </div>
            <div class="inspector-row multi">
              <span>Columns</span>
              <span>{selectedEdge.sourceColumnIds.join(', ')} → {selectedEdge.targetColumnIds.join(', ')}</span>
            </div>
            <div class="inspector-row">
              <span>On update</span>
              <span>{selectedEdge.onUpdate || '—'}</span>
            </div>
            <div class="inspector-row">
              <span>On delete</span>
              <span>{selectedEdge.onDelete || '—'}</span>
            </div>
          </div>
        {:else}
          <div class="diagram-empty">Select a relationship to inspect it.</div>
        {/if}
      </aside>
    </div>
  </div>
{/if}

<style>
  .diagram-shell {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg-editor);
  }

  .diagram-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-panel);
  }

  .toolbar-left,
  .toolbar-actions {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .toolbar-left {
    min-width: 0;
    flex: 1;
  }

  .toolbar-title {
    font-weight: 600;
  }

  .toolbar-meta,
  .zoom-label {
    color: var(--text-muted);
    font-size: 0.92em;
  }

  .diagram-filter {
    min-width: 220px;
    max-width: 320px;
    width: 100%;
    border: 1px solid var(--border);
    background: var(--bg-surface);
    color: var(--text);
    border-radius: 6px;
    padding: 7px 10px;
  }

  .toolbar-actions button {
    border: 1px solid var(--border);
    background: var(--bg-surface);
    color: var(--text);
    border-radius: 6px;
    padding: 6px 10px;
    cursor: pointer;
  }

  .diagram-body {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 320px;
    min-height: 0;
    flex: 1;
  }

  .diagram-canvas {
    position: relative;
    overflow: hidden;
    min-height: 0;
    cursor: grab;
    background:
      radial-gradient(circle at 1px 1px, rgba(255, 255, 255, 0.06) 1px, transparent 0),
      linear-gradient(180deg, rgba(255, 255, 255, 0.02), transparent 45%);
    background-size: 24px 24px, 100% 100%;
  }

  .diagram-canvas:active {
    cursor: grabbing;
  }

  .diagram-stage {
    position: absolute;
    left: 0;
    top: 0;
    transform-origin: top left;
  }

  .diagram-edges {
    position: absolute;
    inset: 0;
    overflow: visible;
    color: rgba(129, 140, 248, 0.65);
    pointer-events: none;
  }

  .diagram-edge {
    fill: none;
    stroke: currentColor;
    stroke-width: 2.5;
    pointer-events: stroke;
    transition: color 120ms ease, stroke-width 120ms ease;
  }

  .diagram-edge.hovered,
  .diagram-edge.selected {
    color: var(--accent-hover);
    stroke-width: 3.5;
  }

  .table-card {
    position: absolute;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: rgba(26, 26, 36, 0.96);
    box-shadow: 0 14px 32px rgba(0, 0, 0, 0.28);
    overflow: hidden;
    user-select: none;
  }

  .table-card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
    background: linear-gradient(180deg, rgba(99, 102, 241, 0.18), rgba(99, 102, 241, 0.08));
    cursor: move;
  }

  .table-card-header h3 {
    margin: 0;
    font-size: 0.96rem;
  }

  .table-card-header p {
    margin: 2px 0 0;
    color: var(--text-muted);
    font-size: 0.78rem;
  }

  .open-query-btn {
    flex-shrink: 0;
    border: 1px solid var(--border);
    background: var(--bg-surface);
    color: var(--text);
    border-radius: 6px;
    padding: 6px 8px;
    cursor: pointer;
  }

  .table-columns {
    display: flex;
    flex-direction: column;
  }

  .column-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-height: 24px;
    padding: 0 12px;
    border-top: 1px solid rgba(255, 255, 255, 0.04);
  }

  .column-row.highlighted {
    background: rgba(129, 140, 248, 0.15);
  }

  .column-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .column-badges {
    display: flex;
    gap: 4px;
  }

  .badge {
    padding: 2px 6px;
    border-radius: 999px;
    font-size: 0.68rem;
    font-weight: 600;
    letter-spacing: 0.02em;
  }

  .badge.pk {
    background: rgba(52, 211, 153, 0.18);
    color: var(--success);
  }

  .badge.fk {
    background: rgba(129, 140, 248, 0.18);
    color: var(--accent-hover);
  }

  .relationship-inspector {
    border-left: 1px solid var(--border);
    background: var(--bg-panel);
    padding: 14px;
    overflow: auto;
  }

  .relationship-inspector h3 {
    margin: 0 0 12px;
    font-size: 0.96rem;
  }

  .inspector-card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg-surface);
  }

  .inspector-name {
    font-weight: 600;
  }

  .inspector-row {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: 0.9rem;
  }

  .inspector-row span:first-child {
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .inspector-row.multi {
    align-items: flex-start;
  }

  .diagram-empty,
  .diagram-notice {
    color: var(--text-muted);
  }

  .diagram-notice {
    position: absolute;
    left: 16px;
    bottom: 16px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: rgba(26, 26, 36, 0.92);
  }

  @media (max-width: 960px) {
    .diagram-body {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: minmax(0, 1fr) auto;
    }

    .relationship-inspector {
      border-left: none;
      border-top: 1px solid var(--border);
    }
  }
</style>