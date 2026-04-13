<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { ExecuteResult } from '../stores/appStore';

  export let result: ExecuteResult | null = null;

  // --- Constants ---
  const ROW_HEIGHT = 28;
  const MAX_SCROLL_HEIGHT = 10_000_000; // 10M px, safely under browser ~33M limit
  const CELL_PAD_X = 10;
  const FONT = '12px -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
  const FONT_SMALL = '11px -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
  const FONT_NULL = 'italic 12px -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';

  // --- DOM refs ---
  let canvas: HTMLCanvasElement;
  let scrollContainer: HTMLDivElement;
  let containerWidth = 0;
  let containerHeight = 0;

  // --- Scroll state ---
  let scrollTop = 0;
  let scrollLeft = 0;

  // --- Theme colours (resolved from CSS vars at mount) ---
  let colors = {
    bg: '#12121a',
    bgPanel: '#1a1a24',
    bgRowAlt: 'rgba(255,255,255,0.02)',
    bgHover: 'rgba(255,255,255,0.04)',
    border: '#2e2e40',
    borderSubtle: '#1e1e2d',
    text: '#e2e2f0',
    textMuted: '#888898',
  };

  // --- Column widths ---
  let colWidths: number[] = [];
  $: if (result?.columns) {
    colWidths = result.columns.map(c => Math.max(c.length * 8 + 24, 100));
  }

  let resizing: { idx: number; startX: number; startW: number } | null = null;

  // --- Sort state ---
  let sortCol = -1;
  let sortDirection: 'asc' | 'desc' = 'asc';
  let sortIndex: number[] | null = null;

  // Reset scroll / sort when a new query starts (result cleared).
  $: if (!result) { scrollTop = 0; scrollLeft = 0; sortCol = -1; sortIndex = null; }

  $: rows = result?.rows ?? [];
  $: totalRows = (result as any)?._rowCount ?? rows.length;
  $: rowNumWidth = Math.max(40, String(totalRows).length * 8 + 16);

  // --- Virtual scroll calculations ---
  $: realTotalHeight = totalRows * ROW_HEIGHT;
  $: useScaledScroll = realTotalHeight > MAX_SCROLL_HEIGHT;
  $: virtualHeight = useScaledScroll ? MAX_SCROLL_HEIGHT : realTotalHeight;
  $: totalContentWidth = rowNumWidth + colWidths.reduce((a, b) => a + b, 0);

  // --- Visible range ---
  let startRow = 0;
  let visibleCount = 0;
  let yOffset = 0;

  $: {
    const _st = scrollTop;
    const _ch = containerHeight;
    const _tr = totalRows;
    const _scaled = useScaledScroll;
    const _vh = virtualHeight;
    const vc = Math.ceil(_ch / ROW_HEIGHT) + 2;

    if (_scaled) {
      const maxScroll = _vh - _ch;
      if (maxScroll > 0) {
        const ratio = _st / maxScroll;
        const maxStart = Math.max(0, _tr - (_ch / ROW_HEIGHT));
        const exactRow = ratio * maxStart;
        startRow = Math.floor(exactRow);
        yOffset = -((exactRow - startRow) * ROW_HEIGHT);
      } else {
        startRow = 0;
        yOffset = 0;
      }
    } else {
      startRow = Math.floor(_st / ROW_HEIGHT);
      yOffset = -(_st % ROW_HEIGHT);
    }
    visibleCount = vc;
  }

  // --- Hover state ---
  let hoveredRow = -1;

  // --- Rendering ---
  let rafId = 0;

  function scheduleRender() {
    if (rafId) return;
    rafId = requestAnimationFrame(() => {
      rafId = 0;
      renderCanvas();
    });
  }

  // Trigger re-render whenever rendering-relevant state changes.
  $: if (canvas && result) {
    void (startRow, visibleCount, yOffset, scrollLeft, containerWidth, containerHeight,
          colWidths, sortIndex, hoveredRow, totalRows, rowNumWidth);
    scheduleRender();
  }

  function resolveColors() {
    const style = getComputedStyle(document.documentElement);
    const g = (name: string) => style.getPropertyValue(name).trim();
    colors = {
      bg: g('--bg') || '#12121a',
      bgPanel: g('--bg-panel') || '#1a1a24',
      bgRowAlt: g('--bg-row-alt') || 'rgba(255,255,255,0.02)',
      bgHover: g('--bg-hover') || 'rgba(255,255,255,0.04)',
      border: g('--border') || '#2e2e40',
      borderSubtle: g('--border-subtle') || '#1e1e2d',
      text: g('--text') || '#e2e2f0',
      textMuted: g('--text-muted') || '#888898',
    };
  }

  function renderCanvas() {
    if (!canvas || !result || containerWidth <= 0 || containerHeight <= 0) return;

    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const w = containerWidth;
    const h = containerHeight;

    // Resize backing store for HiDPI
    const bw = Math.round(w * dpr);
    const bh = Math.round(h * dpr);
    if (canvas.width !== bw || canvas.height !== bh) {
      canvas.width = bw;
      canvas.height = bh;
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    // Clear
    ctx.fillStyle = colors.bg;
    ctx.fillRect(0, 0, w, h);

    const _rows = rows;
    const _idx = sortIndex;
    const _tr = totalRows;
    const _cw = colWidths;
    const _rnw = rowNumWidth;
    const _sl = scrollLeft;
    const _sr = startRow;
    const _yo = yOffset;
    const _vc = visibleCount;
    const _hr = hoveredRow;
    const numCols = _cw.length;

    // Pre-compute column x positions (before scroll offset)
    const colX: number[] = new Array(numCols);
    let cx = _rnw;
    for (let c = 0; c < numCols; c++) { colX[c] = cx; cx += _cw[c]; }

    // Horizontal column virtualization
    let colStart = 0;
    let colEnd = numCols;
    {
      const viewLeft = _sl;
      const viewRight = _sl + w;
      for (let c = 0; c < numCols; c++) {
        if (colX[c] + _cw[c] > viewLeft) { colStart = c; break; }
      }
      for (let c = numCols - 1; c >= 0; c--) {
        if (colX[c] < viewRight) { colEnd = c + 1; break; }
      }
      colStart = Math.max(0, colStart - 1);
      colEnd = Math.min(numCols, colEnd + 1);
    }

    // Full-height row-number column background
    ctx.fillStyle = colors.bgPanel;
    ctx.fillRect(0, 0, _rnw, h);
    ctx.fillStyle = colors.border;
    ctx.fillRect(_rnw - 1, 0, 1, h);

    ctx.textBaseline = 'middle';

    // Draw rows
    for (let i = 0; i < _vc; i++) {
      const absRow = _sr + i;
      if (absRow >= _tr) break;

      const rowDataIdx = _idx && absRow < _idx.length ? _idx[absRow] : absRow;
      const row = _rows[rowDataIdx];
      if (!row) continue;

      const y = _yo + i * ROW_HEIGHT;
      if (y + ROW_HEIGHT < 0 || y > h) continue;

      // Row background (cell area only, right of row-num)
      if (absRow === _hr) {
        ctx.fillStyle = colors.bgHover;
        ctx.fillRect(_rnw, y, w - _rnw, ROW_HEIGHT);
      } else if (absRow % 2 === 1) {
        ctx.fillStyle = colors.bgRowAlt;
        ctx.fillRect(_rnw, y, w - _rnw, ROW_HEIGHT);
      }

      // Row bottom border (full width)
      ctx.fillStyle = colors.borderSubtle;
      ctx.fillRect(0, y + ROW_HEIGHT - 1, w, 1);

      // Cell content (visible columns only)
      ctx.font = FONT;
      for (let c = colStart; c < colEnd; c++) {
        const cw = _cw[c];
        const x = colX[c] - _sl;

        // Column right border
        ctx.fillStyle = colors.borderSubtle;
        ctx.fillRect(x + cw - 1, y, 1, ROW_HEIGHT);

        // Cell text (clipped to column)
        const textMaxW = cw - CELL_PAD_X * 2;
        if (textMaxW <= 0) continue;

        ctx.save();
        ctx.beginPath();
        ctx.rect(x + 1, y, cw - 2, ROW_HEIGHT);
        ctx.clip();

        const cell = row[c];
        if (cell === null) {
          ctx.fillStyle = colors.textMuted;
          ctx.globalAlpha = 0.6;
          ctx.font = FONT_NULL;
          ctx.fillText('NULL', x + CELL_PAD_X, y + ROW_HEIGHT / 2);
          ctx.globalAlpha = 1;
          ctx.font = FONT;
        } else {
          ctx.fillStyle = colors.text;
          ctx.fillText(String(cell), x + CELL_PAD_X, y + ROW_HEIGHT / 2);
        }

        ctx.restore();
      }

      // Row number (painted on top so it acts as a sticky column)
      ctx.fillStyle = colors.bgPanel;
      ctx.fillRect(0, y, _rnw, ROW_HEIGHT);
      ctx.fillStyle = colors.border;
      ctx.fillRect(_rnw - 1, y, 1, ROW_HEIGHT);

      ctx.fillStyle = colors.textMuted;
      ctx.font = FONT_SMALL;
      ctx.textAlign = 'right';
      ctx.fillText(String(absRow + 1), _rnw - 8, y + ROW_HEIGHT / 2);
      ctx.textAlign = 'left';
      ctx.font = FONT;
    }
  }

  // --- Scroll handler ---
  function onScroll(e: Event) {
    const el = e.target as HTMLDivElement;
    scrollTop = el.scrollTop;
    scrollLeft = el.scrollLeft;
  }

  // --- Mouse handlers (hover & copy) ---
  function onCanvasMouseMove(e: MouseEvent) {
    const rect = canvas.getBoundingClientRect();
    const y = e.clientY - rect.top;
    const rowInView = Math.floor((y - yOffset) / ROW_HEIGHT);
    const absRow = startRow + rowInView;
    const next = absRow >= 0 && absRow < totalRows ? absRow : -1;
    if (next !== hoveredRow) hoveredRow = next;
  }

  function onCanvasMouseLeave() {
    if (hoveredRow !== -1) hoveredRow = -1;
  }

  function getCellFromMouse(e: MouseEvent): { row: number; col: number } | null {
    const rect = canvas.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    const rowInView = Math.floor((my - yOffset) / ROW_HEIGHT);
    const absRow = startRow + rowInView;
    if (absRow < 0 || absRow >= totalRows) return null;

    const xInContent = mx - rowNumWidth + scrollLeft;
    if (xInContent < 0) return null;

    let cumW = 0;
    for (let c = 0; c < colWidths.length; c++) {
      cumW += colWidths[c];
      if (xInContent < cumW) return { row: absRow, col: c };
    }
    return null;
  }

  function onCanvasDblClick(e: MouseEvent) {
    const hit = getCellFromMouse(e);
    if (!hit) return;
    const rowDataIdx = sortIndex && hit.row < sortIndex.length ? sortIndex[hit.row] : hit.row;
    const row = rows[rowDataIdx];
    if (!row) return;
    copyCell(row[hit.col]);
  }

  // --- Utility functions ---
  function copyCell(val: any) {
    const text = val === null ? 'NULL' : String(val);
    navigator.clipboard.writeText(text).catch(() => {});
  }

  function copyRow(row: any[]) {
    navigator.clipboard.writeText(row.map(v => v === null ? 'NULL' : String(v)).join('\t')).catch(() => {});
  }

  function escapeCSV(value: any): string {
    if (value === null) return '';
    const str = String(value);
    if (str.includes(',') || str.includes('"') || str.includes('\n') || str.includes('\r')) {
      return '"' + str.replace(/"/g, '""') + '"';
    }
    return str;
  }

  function exportToCSV() {
    if (!result || !result.columns || !result.rows) return;

    const n = rows.length;
    const csvRows = [
      result.columns.map(col => escapeCSV(col)).join(','),
      ...Array.from({ length: n }, (_, i) => {
        const row = sortIndex ? rows[sortIndex[i]] : rows[i];
        return row.map(cell => escapeCSV(cell)).join(',');
      }),
    ];

    const csvContent = csvRows.join('\n');
    const blob = new Blob([csvContent], { type: 'text/csv;charset=utf-8;' });
    const link = document.createElement('a');

    if (link.download !== undefined) {
      const url = URL.createObjectURL(blob);
      link.setAttribute('href', url);
      link.setAttribute('download', 'query_results.csv');
      link.style.visibility = 'hidden';
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
      URL.revokeObjectURL(url);
    }
  }

  // --- Column resize ---
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

  // --- Sort ---
  function buildSortIndex() {
    if (sortCol < 0 || rows.length === 0) { sortIndex = null; return; }
    const dir = sortDirection === 'asc' ? 1 : -1;
    const col = sortCol;
    const n = rows.length;
    const idx = new Array<number>(n);
    for (let i = 0; i < n; i++) idx[i] = i;
    idx.sort((a, b) => {
      const av = rows[a][col]; const bv = rows[b][col];
      if (av === null && bv === null) return 0;
      if (av === null) return dir;
      if (bv === null) return -dir;
      return String(av).localeCompare(String(bv), undefined, { numeric: true }) * dir;
    });
    sortIndex = idx;
  }

  function toggleSort(idx: number) {
    if (sortCol === idx) {
      sortDirection = sortDirection === 'asc' ? 'desc' : 'asc';
    } else {
      sortCol = idx;
      sortDirection = 'asc';
    }
    buildSortIndex();
  }

  // Reset scroll when a new result set arrives (columns reference changes).
  let _lastColumns: string[] | null = null;
  $: if (result && scrollContainer && result.columns !== _lastColumns) {
    _lastColumns = result.columns;
    scrollContainer.scrollTop = 0;
    scrollContainer.scrollLeft = 0;
    scrollLeft = 0;
  }

  // --- Lifecycle ---
  onMount(() => {
    resolveColors();
    scheduleRender();
  });

  onDestroy(() => {
    if (rafId) cancelAnimationFrame(rafId);
  });
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
    <div class="grid-header">
      <div
        class="row-num-header"
        style="width:{rowNumWidth}px; min-width:{rowNumWidth}px"
        role="columnheader"
      >#</div>
      <div class="header-scroll">
        <div class="header-cells" style="transform: translateX({-scrollLeft}px);">
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
      </div>
    </div>

    <!-- Canvas body -->
    <div
      class="grid-body"
      bind:this={scrollContainer}
      bind:clientWidth={containerWidth}
      bind:clientHeight={containerHeight}
      on:scroll={onScroll}
      role="grid"
      aria-rowcount={totalRows}
    >
      <div style="width:{totalContentWidth}px; height:{virtualHeight}px; position: relative;">
        <canvas
          bind:this={canvas}
          style="position: sticky; top: 0; left: 0; width:{containerWidth}px; height:{containerHeight}px; display: block;"
          on:mousemove={onCanvasMouseMove}
          on:mouseleave={onCanvasMouseLeave}
          on:dblclick={onCanvasDblClick}
        ></canvas>
      </div>
    </div>
    <span class="sr-only">{totalRows} rows</span>
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
    flex-shrink: 0;
    font-size: 12px; font-weight: 600; color: var(--text-muted);
  }
  .row-num-header {
    flex-shrink: 0;
    display: flex; align-items: center; justify-content: center;
    padding: 6px 8px; font-size: 11px; color: var(--text-muted);
    border-right: 1px solid var(--border);
    user-select: none;
  }
  .header-scroll {
    flex: 1; overflow: hidden;
  }
  .header-cells {
    display: flex; will-change: transform;
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

  .grid-body { flex: 1; overflow: auto; }

  .sr-only {
    position: absolute; width: 1px; height: 1px;
    overflow: hidden; clip: rect(0,0,0,0);
    padding: 0; margin: -1px; border: 0;
  }
</style>
