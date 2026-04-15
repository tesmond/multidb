<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { ExecuteResult } from '../stores/appStore';
  import { tabs } from '../stores/appStore';
  import { exportResultToCSV } from '../lib/csv';

  export let result: ExecuteResult | null = null;
  export let tabId: string = '';

  // --- Constants ---
  const ROW_HEIGHT = 28;
  const MAX_SCROLL_HEIGHT = 10_000_000;
  const CELL_PAD_X = 10;
  const FONT = '12px -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
  const FONT_SMALL = '11px -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
  const FONT_NULL = 'italic 12px -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
  // Average character width at 12px sans-serif — used for content-width estimation
  const AVG_CHAR_W = 7;

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
    bgSel: 'rgba(70,130,255,0.28)',
    border: '#2e2e40',
    borderSubtle: '#1e1e2d',
    borderSel: 'rgba(80,140,255,0.85)',
    text: '#e2e2f0',
    textMuted: '#888898',
  };

  // ─── Column widths ────────────────────────────────────────────────────────────
  // Svelte's safe_not_equal always considers object props as changed, so a
  // naive `$: if (result?.columns)` resets widths on every sort/tab-store update.
  // We guard with a string key so widths only reset when column names truly change.
  let colWidths: number[] = [];
  let _colWidthsKey = '';

  $: {
    const key = result?.columns ? result.columns.join('\x00') : '';
    if (key !== _colWidthsKey) {
      _colWidthsKey = key;
      colWidths = result?.columns
        ? result.columns.map(c => Math.max(c.length * AVG_CHAR_W + CELL_PAD_X * 2 + 16, 100))
        : [];
    }
  }

  // ─── Column max widths (for auto-fit on resize-handle double-click) ───────────
  // Scanned once per result set and cached so repeated double-clicks are instant.
  let colMaxWidths: number[] = [];
  let _colMaxKey = '';

  $: {
    const key = result?.columns ? result.columns.join('\x00') : '';
    if (key !== _colMaxKey) {
      _colMaxKey = key;
      const cols = result?.columns;
      const dataRows = result?.rows ?? [];
      if (!cols || cols.length === 0) {
        colMaxWidths = [];
      } else {
        // Seed with header label width (leave room for the sort icon too)
        const maxW = cols.map(c => c.length * AVG_CHAR_W + CELL_PAD_X * 2 + 22);
        for (const row of dataRows) {
          for (let c = 0; c < row.length && c < cols.length; c++) {
            const v = row[c];
            const len = v === null ? 4 : String(v).length; // NULL = 4 chars
            const w = len * AVG_CHAR_W + CELL_PAD_X * 2;
            if (w > maxW[c]) maxW[c] = w;
          }
        }
        colMaxWidths = maxW.map(w => Math.min(Math.max(w, 50), 800));
      }
    }
  }

  // ─── Resize drag state ────────────────────────────────────────────────────────
  let resizing: { idx: number; startX: number; startW: number } | null = null;
  // Tracks whether the mouse actually moved during a resize drag so we can
  // suppress the click-to-sort that fires on mouseup after the drag ends.
  let didResize = false;

  // ─── Sort state (persisted per tab via the store) ─────────────────────────────
  $: sortCol = $tabs.find(t => t.id === tabId)?.sortCol ?? -1;
  $: sortDirection = ($tabs.find(t => t.id === tabId)?.sortDirection ?? 'asc') as 'asc' | 'desc';

  $: sortIndex = (() => {
    if (sortCol < 0 || rows.length === 0) return null;
    const dir = sortDirection === 'asc' ? 1 : -1;
    const col = sortCol;
    const n = rows.length;
    const idx = new Array<number>(n);
    for (let i = 0; i < n; i++) idx[i] = i;
    idx.sort((a, b) => {
      const av = rows[a][col];
      const bv = rows[b][col];
      if (av === null && bv === null) return 0;
      if (av === null) return dir;
      if (bv === null) return -dir;
      return String(av).localeCompare(String(bv), undefined, { numeric: true }) * dir;
    });
    return idx;
  })();

  // ─── Selection state ──────────────────────────────────────────────────────────
  // r0/c0 = anchor (where drag started); r1/c1 = current drag end.
  // Coordinates are in visual (post-sort) row order.
  let sel: { r0: number; c0: number; r1: number; c1: number } | null = null;
  let isSelecting = false;
  let selAnchor: { row: number; col: number } | null = null;

  // ─── Reset when result is cleared ────────────────────────────────────────────
  $: if (!result) { scrollTop = 0; scrollLeft = 0; sel = null; }

  $: rows = result?.rows ?? [];
  $: totalRows = (result as any)?._rowCount ?? rows.length;
  $: rowNumWidth = Math.max(40, String(totalRows).length * 8 + 16);

  // ─── Virtual scroll ───────────────────────────────────────────────────────────
  $: realTotalHeight = totalRows * ROW_HEIGHT;
  $: useScaledScroll = realTotalHeight > MAX_SCROLL_HEIGHT;
  $: virtualHeight = useScaledScroll ? MAX_SCROLL_HEIGHT : realTotalHeight;
  $: totalContentWidth = rowNumWidth + colWidths.reduce((a, b) => a + b, 0);

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
        const maxStart = Math.max(0, _tr - _ch / ROW_HEIGHT);
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

  // ─── Hover state ──────────────────────────────────────────────────────────────
  let hoveredRow = -1;

  // ─── Render scheduling ────────────────────────────────────────────────────────
  let rafId = 0;

  function scheduleRender() {
    if (rafId) return;
    rafId = requestAnimationFrame(() => {
      rafId = 0;
      renderCanvas();
    });
  }

  // Re-render whenever any visible state changes — sel is included so selection
  // updates immediately without waiting for the next scroll/hover change.
  $: if (canvas && result) {
    void (startRow, visibleCount, yOffset, scrollLeft, containerWidth, containerHeight,
          colWidths, sortIndex, hoveredRow, totalRows, rowNumWidth, sel);
    scheduleRender();
  }

  function resolveColors() {
    const style = getComputedStyle(document.documentElement);
    const g = (v: string) => style.getPropertyValue(v).trim();
    colors = {
      bg:          g('--bg')          || '#12121a',
      bgPanel:     g('--bg-panel')    || '#1a1a24',
      bgRowAlt:    g('--bg-row-alt')  || 'rgba(255,255,255,0.02)',
      bgHover:     g('--bg-hover')    || 'rgba(255,255,255,0.04)',
      bgSel:       'rgba(70,130,255,0.28)',
      border:      g('--border')      || '#2e2e40',
      borderSubtle:g('--border-subtle')|| '#1e1e2d',
      borderSel:   'rgba(80,140,255,0.85)',
      text:        g('--text')        || '#e2e2f0',
      textMuted:   g('--text-muted')  || '#888898',
    };
  }

  // ─── Canvas renderer ──────────────────────────────────────────────────────────
  function renderCanvas() {
    if (!canvas || !result || containerWidth <= 0 || containerHeight <= 0) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const w = containerWidth;
    const h = containerHeight;
    const bw = Math.round(w * dpr);
    const bh = Math.round(h * dpr);
    if (canvas.width !== bw || canvas.height !== bh) {
      canvas.width = bw;
      canvas.height = bh;
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    ctx.fillStyle = colors.bg;
    ctx.fillRect(0, 0, w, h);

    const _rows   = rows;
    const _idx    = sortIndex;
    const _tr     = totalRows;
    const _cw     = colWidths;
    const _rnw    = rowNumWidth;
    const _sl     = scrollLeft;
    const _sr     = startRow;
    const _yo     = yOffset;
    const _vc     = visibleCount;
    const _hr     = hoveredRow;
    const _sel    = sel;
    const numCols = _cw.length;

    // Pre-compute column x positions (content-space, before scroll)
    const colX: number[] = new Array(numCols);
    let cx = _rnw;
    for (let c = 0; c < numCols; c++) { colX[c] = cx; cx += _cw[c]; }

    // Horizontal column virtualisation
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
      colEnd   = Math.min(numCols, colEnd + 1);
    }

    // Normalise selection once
    let selR0 = -1, selR1 = -1, selC0 = -1, selC1 = -1;
    if (_sel) {
      selR0 = Math.min(_sel.r0, _sel.r1);
      selR1 = Math.max(_sel.r0, _sel.r1);
      selC0 = Math.min(_sel.c0, _sel.c1);
      selC1 = Math.max(_sel.c0, _sel.c1);
    }

    // Row-number sticky column
    ctx.fillStyle = colors.bgPanel;
    ctx.fillRect(0, 0, _rnw, h);
    ctx.fillStyle = colors.border;
    ctx.fillRect(_rnw - 1, 0, 1, h);

    ctx.textBaseline = 'middle';

    // ── Draw rows ──────────────────────────────────────────────────────────────
    for (let i = 0; i < _vc; i++) {
      const absRow = _sr + i;
      if (absRow >= _tr) break;

      const rowDataIdx = _idx && absRow < _idx.length ? _idx[absRow] : absRow;
      const row = _rows[rowDataIdx];
      if (!row) continue;

      const y = _yo + i * ROW_HEIGHT;
      if (y + ROW_HEIGHT < 0 || y > h) continue;

      const inRowSel = _sel !== null && absRow >= selR0 && absRow <= selR1;

      // Row background (painted before per-cell selection overlay)
      if (absRow === _hr) {
        ctx.fillStyle = colors.bgHover;
        ctx.fillRect(_rnw, y, w - _rnw, ROW_HEIGHT);
      } else if (absRow % 2 === 1) {
        ctx.fillStyle = colors.bgRowAlt;
        ctx.fillRect(_rnw, y, w - _rnw, ROW_HEIGHT);
      }

      // Row bottom border
      ctx.fillStyle = colors.borderSubtle;
      ctx.fillRect(0, y + ROW_HEIGHT - 1, w, 1);

      // Cells
      ctx.font = FONT;
      for (let c = colStart; c < colEnd; c++) {
        const cw = _cw[c];
        const x  = colX[c] - _sl;

        // Column right border
        ctx.fillStyle = colors.borderSubtle;
        ctx.fillRect(x + cw - 1, y, 1, ROW_HEIGHT);

        // Selection / hover highlight per cell
        if (inRowSel && c >= selC0 && c <= selC1) {
          // Cell is inside the selection rectangle
          ctx.fillStyle = colors.bgSel;
          ctx.fillRect(x, y, cw - 1, ROW_HEIGHT - 1);
        } else if (inRowSel) {
          // Row is selected but this column is outside the selection —
          // repaint with the normal row background to "un-highlight" it
          ctx.fillStyle =
            absRow === _hr  ? colors.bgHover   :
            absRow % 2 === 1 ? colors.bgRowAlt : colors.bg;
          ctx.fillRect(x, y, cw - 1, ROW_HEIGHT - 1);
        }

        // Cell text (clipped to column bounds)
        const textMaxW = cw - CELL_PAD_X * 2;
        if (textMaxW <= 0) continue;

        ctx.save();
        ctx.beginPath();
        ctx.rect(x + 1, y, cw - 2, ROW_HEIGHT);
        ctx.clip();

        const cell = row[c];
        if (cell === null) {
          ctx.fillStyle   = colors.textMuted;
          ctx.globalAlpha = 0.6;
          ctx.font        = FONT_NULL;
          ctx.fillText('NULL', x + CELL_PAD_X, y + ROW_HEIGHT / 2);
          ctx.globalAlpha = 1;
          ctx.font        = FONT;
        } else {
          ctx.fillStyle = colors.text;
          ctx.fillText(String(cell), x + CELL_PAD_X, y + ROW_HEIGHT / 2);
        }
        ctx.restore();
      }

      // Row-number column (painted on top so it's sticky / always visible)
      ctx.fillStyle = colors.bgPanel;
      ctx.fillRect(0, y, _rnw, ROW_HEIGHT);
      ctx.fillStyle = colors.border;
      ctx.fillRect(_rnw - 1, y, 1, ROW_HEIGHT);

      ctx.fillStyle   = colors.textMuted;
      ctx.font        = FONT_SMALL;
      ctx.textAlign   = 'right';
      ctx.fillText(String(absRow + 1), _rnw - 8, y + ROW_HEIGHT / 2);
      ctx.textAlign   = 'left';
      ctx.font        = FONT;
    }

    // ── Selection border ───────────────────────────────────────────────────────
    if (_sel && selR0 >= 0 && selC0 >= 0 && selC0 < numCols && selC1 < numCols) {
      const topVisRow = Math.max(selR0, _sr);
      const botVisRow = Math.min(selR1, _sr + _vc - 1);

      if (topVisRow <= botVisRow) {
        const yt = _yo + (topVisRow - _sr) * ROW_HEIGHT;
        const yb = _yo + (botVisRow - _sr + 1) * ROW_HEIGHT - 1;
        const xl = colX[selC0] - _sl;
        const xr = colX[selC1] - _sl + _cw[selC1] - 1;

        ctx.save();
        // Clip to the data area (right of row-number column)
        ctx.beginPath();
        ctx.rect(_rnw, 0, w - _rnw, h);
        ctx.clip();

        ctx.strokeStyle = colors.borderSel;
        ctx.lineWidth   = 1.5;
        ctx.strokeRect(xl + 0.75, yt + 0.75, xr - xl - 1.5, yb - yt - 1.5);
        ctx.restore();
      }
    }
  }

  // ─── Scroll handler ───────────────────────────────────────────────────────────
  function onScroll(e: Event) {
    const el = e.target as HTMLDivElement;
    scrollTop  = el.scrollTop;
    scrollLeft = el.scrollLeft;
  }

  // ─── Hit-testing ─────────────────────────────────────────────────────────────
  function getCellFromMouse(e: MouseEvent): { row: number; col: number } | null {
    if (!canvas) return null;
    const rect = canvas.getBoundingClientRect();
    const mx   = e.clientX - rect.left;
    const my   = e.clientY - rect.top;

    // Ignore clicks in the row-number column
    if (mx < rowNumWidth) return null;

    const rowInView = Math.floor((my - yOffset) / ROW_HEIGHT);
    const absRow    = startRow + rowInView;
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

  // ─── Canvas mouse: selection ──────────────────────────────────────────────────
  function onCanvasMouseDown(e: MouseEvent) {
    if (e.button !== 0) return;
    const hit = getCellFromMouse(e);
    if (!hit) { sel = null; scheduleRender(); return; }

    isSelecting = true;
    selAnchor   = hit;
    sel         = { r0: hit.row, c0: hit.col, r1: hit.row, c1: hit.col };
    scheduleRender();

    // mouseup may occur outside the canvas during a fast drag
    window.addEventListener('mouseup', onWindowSelectionMouseUp);
  }

  function onCanvasMouseMove(e: MouseEvent) {
    // Hover tracking
    if (canvas) {
      const rect      = canvas.getBoundingClientRect();
      const my        = e.clientY - rect.top;
      const rowInView = Math.floor((my - yOffset) / ROW_HEIGHT);
      const absRow    = startRow + rowInView;
      const next      = absRow >= 0 && absRow < totalRows ? absRow : -1;
      if (next !== hoveredRow) hoveredRow = next;
    }

    // Extend selection while the primary button is held
    if (isSelecting && selAnchor) {
      const hit = getCellFromMouse(e);
      if (hit && sel && (hit.row !== sel.r1 || hit.col !== sel.c1)) {
        sel = { r0: selAnchor.row, c0: selAnchor.col, r1: hit.row, c1: hit.col };
        scheduleRender();
      }
    }
  }

  function onCanvasMouseLeave() {
    if (hoveredRow !== -1) hoveredRow = -1;
    // Do not cancel selection — the user may still be dragging
  }

  function onWindowSelectionMouseUp() {
    isSelecting = false;
    selAnchor   = null;
    window.removeEventListener('mouseup', onWindowSelectionMouseUp);
  }

  // Double-click: copy single cell value and collapse selection to that cell
  function onCanvasDblClick(e: MouseEvent) {
    const hit = getCellFromMouse(e);
    if (!hit) return;

    const rowDataIdx = sortIndex && hit.row < sortIndex.length ? sortIndex[hit.row] : hit.row;
    const row        = rows[rowDataIdx];
    if (!row) return;

    const val  = row[hit.col];
    const text = val === null ? 'NULL' : String(val);
    navigator.clipboard.writeText(text).catch(() => {});

    sel = { r0: hit.row, c0: hit.col, r1: hit.row, c1: hit.col };
    scheduleRender();
  }

  // ─── Keyboard: copy selection / clear ────────────────────────────────────────
  function onGridKeyDown(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key === 'c') {
      e.preventDefault();
      copySelection();
    }
    if (e.key === 'Escape') {
      sel = null;
      scheduleRender();
    }
  }

  function copySelection() {
    if (!sel || !result) return;

    const r0 = Math.min(sel.r0, sel.r1);
    const r1 = Math.max(sel.r0, sel.r1);
    const c0 = Math.min(sel.c0, sel.c1);
    const c1 = Math.max(sel.c0, sel.c1);

    const lines: string[] = [];
    for (let r = r0; r <= r1; r++) {
      const rowDataIdx = sortIndex && r < sortIndex.length ? sortIndex[r] : r;
      const row        = rows[rowDataIdx];
      if (!row) continue;
      const cells: string[] = [];
      for (let c = c0; c <= c1; c++) {
        const v = c < row.length ? row[c] : null;
        cells.push(v === null ? '' : String(v));
      }
      lines.push(cells.join(','));
    }

    navigator.clipboard.writeText(lines.join('\n')).catch(() => {});
  }

  // ─── Column resize (drag) ─────────────────────────────────────────────────────
  function startResize(e: MouseEvent, idx: number) {
    e.preventDefault();
    e.stopPropagation();
    didResize = false;
    resizing  = { idx, startX: e.clientX, startW: colWidths[idx] };
    window.addEventListener('mousemove', onResize);
    window.addEventListener('mouseup',  stopResize);
  }

  function onResize(e: MouseEvent) {
    if (!resizing) return;
    const delta = e.clientX - resizing.startX;
    if (Math.abs(delta) > 2) didResize = true;
    colWidths[resizing.idx] = Math.max(50, resizing.startW + delta);
    colWidths = [...colWidths];
  }

  function stopResize() {
    resizing = null;
    window.removeEventListener('mousemove', onResize);
    window.removeEventListener('mouseup',  stopResize);
  }

  // ─── Auto-fit column on resize-handle double-click ───────────────────────────
  // Uses the precomputed colMaxWidths so there is no re-scan of the data.
  function autoFitColumn(e: MouseEvent, idx: number) {
    e.preventDefault();
    e.stopPropagation();
    if (colMaxWidths[idx] != null) {
      colWidths[idx] = colMaxWidths[idx];
      colWidths = [...colWidths];
    }
  }

  // ─── Sort ─────────────────────────────────────────────────────────────────────
  function toggleSort(idx: number) {
    if (!tabId) return;
    const newDir: 'asc' | 'desc' = sortCol === idx
      ? (sortDirection === 'asc' ? 'desc' : 'asc')
      : 'asc';
    tabs.updateTab(tabId, { sortCol: idx, sortDirection: newDir });
  }

  // Wrapper: swallow the click if it was actually the end of a resize drag
  function onHeaderClick(idx: number) {
    if (didResize) { didResize = false; return; }
    toggleSort(idx);
  }

  // ─── Reset on new result ──────────────────────────────────────────────────────
  let _lastColumns: string[] | null = null;
  $: if (result && result.columns !== _lastColumns) {
    _lastColumns = result.columns;
    if (scrollContainer) {
      scrollContainer.scrollTop  = 0;
      scrollContainer.scrollLeft = 0;
      scrollLeft = 0;
    }
    sel = null;
    if (tabId) tabs.updateTab(tabId, { sortCol: -1, sortDirection: 'asc' });
  }

  // ─── Lifecycle ────────────────────────────────────────────────────────────────
  onMount(() => {
    resolveColors();
    scheduleRender();
  });

  onDestroy(() => {
    if (rafId) cancelAnimationFrame(rafId);
    window.removeEventListener('mouseup',   onWindowSelectionMouseUp);
    window.removeEventListener('mousemove', onResize);
    window.removeEventListener('mouseup',   stopResize);
  });
</script>

{#if !result}
  <div class="empty">Run a query to see results here.</div>
{:else if result.error}
  <div class="empty error">{result.error}</div>
{:else if result.columns.length === 0}
  <div class="empty">Query executed. {result.rowsAffected} row(s) affected in {result.duration}ms.</div>
{:else}
  <!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
  <!-- svelte-ignore a11y-no-noninteractive-tabindex -->
  <div
    class="grid-wrap"
    tabindex="0"
    role="region"
    aria-label="Query results"
    on:keydown={onGridKeyDown}
  >
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
              on:click={() => onHeaderClick(i)}
              role="columnheader"
              aria-sort={sortCol === i ? (sortDirection === 'asc' ? 'ascending' : 'descending') : 'none'}
              tabindex="0"
              on:keydown={e => e.key === 'Enter' && onHeaderClick(i)}
            >
              <span class="col-label">{col}</span>
              {#if sortCol === i}
                <span class="sort-icon">{sortDirection === 'asc' ? '▲' : '▼'}</span>
              {/if}
              <!-- resize handle: drag to resize, double-click to auto-fit -->
              <div
                class="resize-handle"
                on:mousedown={e => startResize(e, i)}
                on:dblclick={e => autoFitColumn(e, i)}
                on:click|stopPropagation={() => {}}
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
          on:mousedown={onCanvasMouseDown}
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

  .grid-wrap {
    display: flex; flex-direction: column;
    height: 100%; overflow: hidden;
    outline: none; /* suppress browser focus ring on the container */
  }

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
  .header-scroll { flex: 1; overflow: hidden; }
  .header-cells  { display: flex; will-change: transform; }

  .header-cell {
    position: relative; display: flex; align-items: center; gap: 4px;
    padding: 6px 10px; cursor: pointer; user-select: none;
    border-right: 1px solid var(--border); flex-shrink: 0;
    white-space: nowrap; overflow: hidden;
  }
  .header-cell:hover { background: var(--bg-hover); color: var(--text); }

  .col-label  { flex: 1; overflow: hidden; text-overflow: ellipsis; }
  .sort-icon  { opacity: 0.7; font-size: 10px; flex-shrink: 0; }

  /* Wider hit-area (6 px) makes double-click easier to land precisely */
  .resize-handle {
    position: absolute; right: 0; top: 0; bottom: 0; width: 6px;
    cursor: col-resize; z-index: 1;
  }
  .resize-handle:hover { background: var(--accent); opacity: 0.5; }

  .grid-body { flex: 1; overflow: auto; }
  /* Cell cursor to hint that data is selectable */
  .grid-body canvas { cursor: cell; display: block; }

  .sr-only {
    position: absolute; width: 1px; height: 1px;
    overflow: hidden; clip: rect(0,0,0,0);
    padding: 0; margin: -1px; border: 0;
  }
</style>
