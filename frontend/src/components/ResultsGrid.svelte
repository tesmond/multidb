<script lang="ts">
  import type { ExecuteResult } from '../stores/appStore';

  export let result: ExecuteResult | null = null;

  // Virtual scrolling
  const ROW_HEIGHT = 28;
  const PAGE = 100;

  let scrollTop = 0;
  let containerHeight = 0;

  $: visibleStart = Math.floor(scrollTop / ROW_HEIGHT);
  $: visibleCount = Math.ceil(containerHeight / ROW_HEIGHT) + 4;
  $: rows = result?.rows ?? [];
  $: visibleRows = rows.slice(visibleStart, visibleStart + visibleCount);
  $: paddingTop = visibleStart * ROW_HEIGHT;
  $: totalHeight = rows.length * ROW_HEIGHT;

  // Column widths (px)
  let colWidths: number[] = [];
  $: {
    if (result?.columns) {
      colWidths = result.columns.map(c => Math.max(c.length * 8 + 24, 100));
    }
  }

  let resizing: { idx: number; startX: number; startW: number } | null = null;

  function startResize(e: MouseEvent, idx: number) {
    e.preventDefault();
    resizing = { idx, startX: e.clientX, startW: colWidths[idx] };
    window.addEventListener('mousemove', onResize);
    window.addEventListener('mouseup', stopResize);
  }

  function onResize(e: MouseEvent) {
    if (!resizing) return;
    const delta = e.clientX - resizing.startX;
    colWidths[resizing.idx] = Math.max(50, resizing.startW + delta);
    colWidths = [...colWidths];
  }

  function stopResize() {
    resizing = null;
    window.removeEventListener('mousemove', onResize);
    window.removeEventListener('mouseup', stopResize);
  }

  function copyCell(val: any) {
    const text = val === null ? 'NULL' : String(val);
    navigator.clipboard.writeText(text).catch(() => {});
  }

  function copyRow(row: any[]) {
    navigator.clipboard.writeText(row.map(v => v === null ? 'NULL' : String(v)).join('\t')).catch(() => {});
  }

  let sortCol = -1;
  let sortDirection: 'asc' | 'desc' = 'asc';
  let sortedRows: any[][] = [];

  $: {
    if (sortCol >= 0 && result?.rows) {
      const dir = sortDirection === 'asc' ? 1 : -1;
      sortedRows = [...result.rows].sort((a, b) => {
        const av = a[sortCol]; const bv = b[sortCol];
        if (av === null && bv === null) return 0;
        if (av === null) return dir;
        if (bv === null) return -dir;
        return String(av).localeCompare(String(bv), undefined, { numeric: true }) * dir;
      });
    } else {
      sortedRows = result?.rows ?? [];
    }
    // reset sortedRows into rows alias
    rows = sortedRows;
  }

  $: rowNumWidth = Math.max(40, String(rows.length).length * 8 + 16);

  function onGridScroll(e: Event) {
    scrollTop = (e.target as HTMLDivElement).scrollTop;
  }

  function toggleSort(idx: number) {
    if (sortCol === idx) {
      sortDirection = sortDirection === 'asc' ? 'desc' : 'asc';
    } else {
      sortCol = idx;
      sortDirection = 'asc';
    }
  }
</script>

{#if !result}
  <div class="empty">Run a query to see results here.</div>
{:else if result.error}
  <div class="empty error">{result.error}</div>
{:else if result.columns.length === 0}
  <div class="empty">Query executed. {result.rowsAffected} row(s) affected in {result.duration}ms.</div>
{:else}
  <div class="grid-wrap">
    <!-- Header row -->
    <div class="grid-header" style="padding-left: {rowNumWidth}px;">
      {#each result.columns as col, i}
        <div
          class="header-cell"
          style="width:{colWidths[i]}px; min-width:{colWidths[i]}px"
          on:click={() => toggleSort(i)}
          role="columnheader"
          aria-sort={sortCol === i ? (sortDirection === 'asc' ? 'ascending' : 'descending') : 'none'}
          tabindex="0"
          on:keydown={e => e.key === 'Enter' && toggleSort(i)}
        >
          <span class="col-label">{col}</span>
          {#if sortCol === i}
            <span class="sort-icon">{sortDirection === 'asc' ? '▲' : '▼'}</span>
          {/if}
          <div
            class="resize-handle"
            on:mousedown={e => startResize(e, i)}
            role="separator"
            aria-label="Resize column"
          ></div>
        </div>
      {/each}
    </div>

    <!-- Scrollable body -->
    <div
      class="grid-scroll"
      on:scroll={onGridScroll}
      bind:clientHeight={containerHeight}
      role="grid"
      aria-rowcount={rows.length}
    >
      <div style="height:{totalHeight}px; position:relative;">
        <div style="transform: translateY({paddingTop}px);">
          {#each visibleRows as row, ri}
            {@const absIdx = visibleStart + ri}
            <div class="grid-row" class:odd={absIdx % 2 === 1} role="row">
              <div class="row-num" style="width:{rowNumWidth}px; min-width:{rowNumWidth}px">{absIdx + 1}</div>
              {#each row as cell, ci}
                <div
                  class="grid-cell"
                  style="width:{colWidths[ci]}px; min-width:{colWidths[ci]}px"
                  title={cell === null ? 'NULL' : String(cell)}
                  on:dblclick={() => copyCell(cell)}
                  role="gridcell"
                  tabindex="-1"
                >
                  {#if cell === null}
                    <span class="null-val">NULL</span>
                  {:else}
                    {String(cell)}
                  {/if}
                </div>
              {/each}
            </div>
          {/each}
        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  .empty {
    display: flex; align-items: center; justify-content: center;
    height: 100%; color: var(--text-muted); font-size: 13px;
  }
  .empty.error { color: var(--error); }
  .grid-wrap { display: flex; flex-direction: column; height: 100%; overflow: hidden; }

  .grid-header {
    display: flex; align-items: stretch;
    background: var(--bg-panel);
    border-bottom: 2px solid var(--border);
    overflow: hidden; flex-shrink: 0;
    font-size: 12px; font-weight: 600; color: var(--text-muted);
  }
  .header-cell {
    position: relative; display: flex; align-items: center; gap: 4px;
    padding: 6px 10px; cursor: pointer; user-select: none;
    border-right: 1px solid var(--border); flex-shrink: 0;
    white-space: nowrap; overflow: hidden;
  }
  .header-cell:hover { background: var(--bg-hover); color: var(--text); }
  .col-label { flex: 1; overflow: hidden; text-overflow: ellipsis; }
  .sort-icon { opacity: 0.7; font-size: 10px; }
  .resize-handle {
    position: absolute; right: 0; top: 0; bottom: 0; width: 4px;
    cursor: col-resize; z-index: 1;
  }
  .resize-handle:hover { background: var(--accent); opacity: 0.5; }

  .grid-scroll { flex: 1; overflow: auto; }

  .grid-row {
    display: flex; align-items: stretch;
    border-bottom: 1px solid var(--border-subtle);
    height: 28px;
  }
  .grid-row.odd { background: var(--bg-row-alt); }
  .grid-row:hover { background: var(--bg-hover); }

  .row-num {
    flex-shrink: 0;
    display: flex; align-items: center; justify-content: flex-end;
    padding: 0 8px; font-size: 11px; color: var(--text-muted);
    border-right: 1px solid var(--border);
    background: var(--bg-panel);
    user-select: none;
  }
  .grid-cell {
    padding: 0 10px; font-size: 12px;
    display: flex; align-items: center;
    border-right: 1px solid var(--border-subtle);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    cursor: default;
  }
  .null-val { color: var(--text-muted); font-style: italic; opacity: 0.6; }
</style>
