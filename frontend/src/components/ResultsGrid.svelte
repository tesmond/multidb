<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { ExecuteResult, TabEditInfo } from '../stores/appStore';
  import { fontScalePercent, tabs, isSqlTab } from '../stores/appStore';
  import { escapeTsvCell, formatValueForClipboard } from '../lib/resultClipboard';
  import { calculateAutoFitColumnWidth } from '../lib/columnSizing';

  export let result: ExecuteResult | null = null;
  export let tabId: string = '';
  export let editInfo: TabEditInfo | null = null;

  // --- Constants ---
  const BASE_ROW_HEIGHT = 28;
  const MAX_SCROLL_HEIGHT = 10_000_000;
  const BASE_CELL_PAD_X = 10;
  // Used only if the browser cannot provide a canvas text measurement context.
  const BASE_AVG_CHAR_W = 5.6;
  const EXPLAIN_DEFAULT_TEXT_LEN = 120;
  const FONT_FAMILY = '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
  const EDGE_ZONE = 50;       // px from scroll edge to start auto-scrolling
  const MAX_EDGE_SPEED = 15;  // max px per frame during edge auto-scroll

  // --- DOM refs ---
  let canvas: HTMLCanvasElement;
  let scrollContainer: HTMLDivElement;
  let gridWrap: HTMLDivElement;
  let containerWidth = 0;
  let containerHeight = 0;
  $: fontScale = $fontScalePercent / 100;
  $: rowHeight = BASE_ROW_HEIGHT * fontScale;
  $: cellPadX = BASE_CELL_PAD_X * fontScale;
  $: font = `${12 * fontScale}px ${FONT_FAMILY}`;
  $: fontSmall = `${11 * fontScale}px ${FONT_FAMILY}`;
  $: fontNull = `italic ${12 * fontScale}px ${FONT_FAMILY}`;

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
    bgRowSel: 'rgba(70,130,255,0.13)',
    bgRowNumSel: 'rgba(70,130,255,0.45)',
    bgDirty: 'rgba(255,200,50,0.15)',
    border: '#2e2e40',
    borderSubtle: '#1e1e2d',
    borderSel: 'rgba(80,140,255,0.85)',
    borderDirty: 'rgba(255,200,50,0.7)',
    text: '#e2e2f0',
    textMuted: '#888898',
  };

  // ─── Column widths ────────────────────────────────────────────────────────────
  // Svelte's safe_not_equal always considers object props as changed, so a
  // naive `$: if (result?.columns)` resets widths on every sort/tab-store update.
  // We guard with a string key so widths only reset when column names truly change.
  let colWidths: number[] = [];
  let _colWidthsKey = '';
  let _colWidthsFontScale = 1;
  interface ColumnTextMeasurement {
    maxLength: number;
    maxBaseWidth: number;
  }

  // Updated incrementally so auto-fit stays correct across streaming chunks.
  let colTextMeasurements: ColumnTextMeasurement[] = [];
  let _measuredRowCount = 0;
  let textMeasurementContext: CanvasRenderingContext2D | null | undefined;

  function measureBaseTextWidth(text: string): number {
    if (textMeasurementContext === undefined) {
      textMeasurementContext = typeof document === 'undefined'
        ? null
        : document.createElement('canvas').getContext('2d');
      if (textMeasurementContext) {
        textMeasurementContext.font = `12px ${FONT_FAMILY}`;
      }
    }
    return textMeasurementContext?.measureText(text).width
      ?? text.length * BASE_AVG_CHAR_W;
  }

  function initialColumnTextLen(columnName: string): number {
    return columnName.toLowerCase() === 'explain'
      ? Math.max(columnName.length, EXPLAIN_DEFAULT_TEXT_LEN)
      : columnName.length;
  }

  // Reset column state when the column set changes (new query).
  $: {
    const key = result?.columns ? result.columns.join('\x00') : '';
    if (key !== _colWidthsKey) {
      _colWidthsKey = key;
      if (result?.columns) {
        colTextMeasurements = result.columns.map(columnName => ({
          maxLength: initialColumnTextLen(columnName),
          maxBaseWidth: 0,
        }));
        colWidths = colTextMeasurements.map((_, idx) => columnTextWidth(idx));
        _colWidthsFontScale = fontScale;
      } else {
        colTextMeasurements = [];
        colWidths = [];
      }
      _measuredRowCount = 0;
    }
  }

  $: if (result?.columns && colWidths.length > 0 && fontScale !== _colWidthsFontScale) {
    const ratio = fontScale / _colWidthsFontScale;
    colWidths = colWidths.map(width => Math.max(50 * fontScale, width * ratio));
    _colWidthsFontScale = fontScale;
  }

  // Incrementally scan only newly-arrived rows so streaming chunks don't cause
  // a full re-scan of all rows on every update.
  $: {
    const dataRows = result?.rows;
    if (dataRows && colTextMeasurements.length > 0) {
      const prev = _measuredRowCount;
      const next = dataRows.length;
      if (next > prev) {
        const measurements = colTextMeasurements;
        for (let r = prev; r < next; r++) {
          const row = dataRows[r];
          for (let c = 0; c < row.length && c < measurements.length; c++) {
            const v = row[c];
            const text = v === null ? 'NULL' : String(v);
            const measurement = measurements[c];
            if (text.length > measurement.maxLength) measurement.maxLength = text.length;
            const width = measureBaseTextWidth(text);
            if (width > measurement.maxBaseWidth) measurement.maxBaseWidth = width;
          }
        }
        _measuredRowCount = next;
        colTextMeasurements = measurements;
      }
    }
  }

  function columnTextWidth(idx: number): number {
    return calculateAutoFitColumnWidth(
      colTextMeasurements[idx]?.maxBaseWidth ?? 0,
      measureBaseTextWidth(result?.columns[idx] ?? ''),
      {
        cellPaddingX: cellPadX,
        fontScale,
      },
    );
  }

  // ─── Resize drag state ────────────────────────────────────────────────────────
  let resizing: { idx: number; startX: number; startW: number } | null = null;
  // Tracks whether the mouse actually moved during a resize drag so we can
  // suppress the click-to-sort that fires on mouseup after the drag ends.
  let didResize = false;

  // ─── Sort state (persisted per tab via the store) ─────────────────────────────
  $: currentSqlTab = (() => {
    const currentTab = $tabs.find(t => t.id === tabId);
    return isSqlTab(currentTab) ? currentTab : null;
  })();
  $: sortCol = currentSqlTab?.sortCol ?? -1;
  $: sortDirection = currentSqlTab?.sortDirection ?? 'asc';

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
  let lastSelectedCell: { row: number; col: number } | null = null;
  let selectedRows = new Set<number>();
  let rowSelectionAnchor: number | null = null;

  // ─── Edge-scroll (drag auto-scroll) ──────────────────────────────────────────
  let edgeScrollRaf = 0;
  let lastDragMouseEvent: MouseEvent | null = null;

  // ─── Reset when result is cleared ────────────────────────────────────────────
  $: if (!result) {
    scrollTop = 0;
    scrollLeft = 0;
    sel = null;
    selectedRows = new Set();
    rowSelectionAnchor = null;
  }

  $: rows = result?.rows ?? [];
  $: totalRows = (result as any)?._rowCount ?? rows.length;
  $: rowNumWidth = Math.max(40 * fontScale, String(totalRows).length * 8 * fontScale + 16 * fontScale);

  // ─── Virtual scroll ───────────────────────────────────────────────────────────
  $: realTotalHeight = totalRows * rowHeight;
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
    const vc = Math.ceil(_ch / rowHeight) + 2;

    if (_scaled) {
      const maxScroll = _vh - _ch;
      if (maxScroll > 0) {
        const ratio = _st / maxScroll;
        const maxStart = Math.max(0, _tr - _ch / rowHeight);
        const exactRow = ratio * maxStart;
        startRow = Math.floor(exactRow);
        yOffset = -((exactRow - startRow) * rowHeight);
      } else {
        startRow = 0;
        yOffset = 0;
      }
    } else {
      startRow = Math.floor(_st / rowHeight);
      yOffset = -(_st % rowHeight);
    }
    visibleCount = vc;
  }

  // ─── Hover state ──────────────────────────────────────────────────────────────
  let hoveredRow = -1;

  // ─── Pending edits (from store) ───────────────────────────────────────────────
  $: pendingEdits = currentSqlTab?.pendingEdits ?? {};

  // ─── Cell edit overlay ────────────────────────────────────────────────────────
  let editOverlay: { row: number; col: number; rowDataIdx: number; value: string; x: number; y: number; w: number } | null = null;
  let editInput: HTMLInputElement;

  // ─── Column header tooltip ────────────────────────────────────────────────────
  let tooltipCol: number | null = null;
  let tooltipX = 0;
  let tooltipY = 0;

  function onHeaderMouseEnter(e: MouseEvent, idx: number) {
    tooltipCol = idx;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    tooltipX = rect.left;
    tooltipY = rect.bottom + 4;
  }

  function onHeaderMouseLeave() {
    tooltipCol = null;
  }

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
          colWidths, sortIndex, hoveredRow, totalRows, rowNumWidth, sel, selectedRows, pendingEdits, editOverlay,
          rowHeight, cellPadX, font, fontSmall, fontNull);
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
      bgRowSel:    'rgba(70,130,255,0.13)',
      bgRowNumSel: 'rgba(70,130,255,0.45)',
      bgDirty:     'rgba(255,200,50,0.15)',
      border:      g('--border')      || '#2e2e40',
      borderSubtle:g('--border-subtle')|| '#1e1e2d',
      borderSel:   'rgba(80,140,255,0.85)',
      borderDirty: 'rgba(255,200,50,0.7)',
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
    const _selectedRows = selectedRows;
    const _pe     = pendingEdits;
    const _eo     = editOverlay;
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

      const y = _yo + i * rowHeight;
      if (y + rowHeight < 0 || y > h) continue;

      const inCellSel   = _sel !== null && absRow >= selR0 && absRow <= selR1;
      const inRowSelSet = _selectedRows.has(absRow);

      // Row background (painted before per-cell selection overlay)
      if (absRow === _hr) {
        ctx.fillStyle = colors.bgHover;
        ctx.fillRect(_rnw, y, w - _rnw, rowHeight);
      } else if (absRow % 2 === 1) {
        ctx.fillStyle = colors.bgRowAlt;
        ctx.fillRect(_rnw, y, w - _rnw, rowHeight);
      }

      if (inRowSelSet) {
        ctx.fillStyle = colors.bgRowSel;
        ctx.fillRect(_rnw, y, w - _rnw, rowHeight - 1);
      }

      // Row bottom border
      ctx.fillStyle = colors.borderSubtle;
      ctx.fillRect(0, y + rowHeight - 1, w, 1);

      // Cells
      ctx.font = font;
      for (let c = colStart; c < colEnd; c++) {
        const cw = _cw[c];
        const x  = colX[c] - _sl;

        // Column right border
        ctx.fillStyle = colors.borderSubtle;
        ctx.fillRect(x + cw - 1, y, 1, rowHeight);

        // Selection / hover highlight per cell
        if (inCellSel && c >= selC0 && c <= selC1) {
          // Cell is inside the selection rectangle
          ctx.fillStyle = colors.bgSel;
          ctx.fillRect(x, y, cw - 1, rowHeight - 1);
        } else if (inCellSel) {
          // Row is selected but this column is outside the selection —
          // repaint with the normal row background to "un-highlight" it
          ctx.fillStyle =
            absRow === _hr   ? colors.bgHover   :
            absRow % 2 === 1 ? colors.bgRowAlt  : colors.bg;
          ctx.fillRect(x, y, cw - 1, rowHeight - 1);
        }

        // Cell text (clipped to column bounds)
        const textMaxW = cw - cellPadX * 2;
        if (textMaxW <= 0) continue;

        // If this cell is currently open in the edit overlay, blank it out so
        // the canvas text doesn't show through the input element.
        if (_eo && rowDataIdx === _eo.rowDataIdx && c === _eo.col) {
          ctx.fillStyle = colors.bgPanel;
          ctx.fillRect(x + 1, y, cw - 2, rowHeight - 1);
          continue;
        }

        // Check for a pending edit for this cell
        const dirtyVal = _pe[String(rowDataIdx)]?.[String(c)];
        const isDirty = dirtyVal !== undefined;

        // Draw dirty cell background (on top of row / selection backgrounds)
        if (isDirty) {
          ctx.fillStyle = colors.bgDirty;
          ctx.fillRect(x, y, cw - 1, rowHeight - 1);
          // Left indicator line
          ctx.fillStyle = colors.borderDirty;
          ctx.fillRect(x, y, 2, rowHeight - 1);
        }

        ctx.save();
        ctx.beginPath();
        ctx.rect(x + 1, y, cw - 2, rowHeight);
        ctx.clip();

        const cell = isDirty ? dirtyVal : row[c];
        if (cell === null || cell === '') {
          if (!isDirty) {
            // Only show NULL for actual null, not empty strings from edits
            if (row[c] === null) {
              ctx.fillStyle   = colors.textMuted;
              ctx.globalAlpha = 0.6;
              ctx.font        = fontNull;
              ctx.fillText('NULL', x + cellPadX, y + rowHeight / 2);
              ctx.globalAlpha = 1;
              ctx.font        = font;
            }
          } else {
            ctx.fillStyle = colors.text;
            ctx.fillText(String(cell), x + cellPadX, y + rowHeight / 2);
          }
        } else {
          ctx.fillStyle = colors.text;
          ctx.fillText(String(cell), x + cellPadX, y + rowHeight / 2);
        }
        ctx.restore();
      }

      // Row-number column (painted on top so it's sticky / always visible)
      ctx.fillStyle = inRowSelSet ? colors.bgRowNumSel : colors.bgPanel;
      ctx.fillRect(0, y, _rnw, rowHeight);
      ctx.fillStyle = colors.border;
      ctx.fillRect(_rnw - 1, y, 1, rowHeight);

      ctx.fillStyle   = colors.textMuted;
      ctx.font        = fontSmall;
      ctx.textAlign   = 'right';
      ctx.fillText(String(absRow + 1), _rnw - 8 * fontScale, y + rowHeight / 2);
      ctx.textAlign   = 'left';
      ctx.font        = font;
    }

    // ── Selection border ───────────────────────────────────────────────────────
    if (_sel && selR0 >= 0 && selC0 >= 0 && selC0 < numCols && selC1 < numCols) {
      const topVisRow = Math.max(selR0, _sr);
      const botVisRow = Math.min(selR1, _sr + _vc - 1);

      if (topVisRow <= botVisRow) {
        const yt = _yo + (topVisRow - _sr) * rowHeight;
        const yb = _yo + (botVisRow - _sr + 1) * rowHeight - 1;
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

  function clamp(n: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, n));
  }

  function getPageRowStep(): number {
    return Math.max(1, Math.floor(containerHeight / rowHeight));
  }

  function setViewportStartRow(start: number) {
    if (!scrollContainer) return;
    const visibleRows = Math.max(1, Math.floor(containerHeight / rowHeight));
    const maxStart = Math.max(0, totalRows - visibleRows);
    const clampedStart = clamp(start, 0, maxStart);

    if (useScaledScroll) {
      const maxScroll = Math.max(0, virtualHeight - containerHeight);
      const ratio = maxStart > 0 ? clampedStart / maxStart : 0;
      scrollContainer.scrollTop = ratio * maxScroll;
    } else {
      scrollContainer.scrollTop = clampedStart * rowHeight;
    }
    scrollTop = scrollContainer.scrollTop;
  }

  function ensureRowVisible(row: number) {
    if (!scrollContainer) return;
    const visibleRows = Math.max(1, Math.floor(containerHeight / rowHeight));
    const top = startRow;
    const bottom = startRow + visibleRows - 1;
    if (row < top) {
      setViewportStartRow(row);
    } else if (row > bottom) {
      setViewportStartRow(row - visibleRows + 1);
    }
  }

  function getColLeft(col: number): number {
    let left = 0;
    for (let i = 0; i < col; i++) left += colWidths[i] ?? 0;
    return left;
  }

  function ensureColVisible(col: number) {
    if (!scrollContainer) return;
    const colLeft = getColLeft(col);
    const colRight = colLeft + (colWidths[col] ?? 0);
    const dataViewportWidth = Math.max(1, containerWidth - rowNumWidth);

    let next = scrollLeft;
    if (colLeft < scrollLeft) {
      next = colLeft;
    } else if (colRight > scrollLeft + dataViewportWidth) {
      next = colRight - dataViewportWidth;
    }

    const maxScrollLeft = Math.max(0, totalContentWidth - containerWidth);
    next = clamp(next, 0, maxScrollLeft);
    if (next !== scrollLeft) {
      scrollContainer.scrollLeft = next;
      scrollLeft = next;
    }
  }

  function moveSelectionBy(dr: number, dc: number, extend: boolean) {
    if (!result?.columns?.length || totalRows <= 0) return;
    const maxRow = totalRows - 1;
    const maxCol = result.columns.length - 1;

    const current = sel
      ? { row: sel.r1, col: sel.c1 }
      : (lastSelectedCell ?? { row: 0, col: 0 });

    const next = {
      row: clamp(current.row + dr, 0, maxRow),
      col: clamp(current.col + dc, 0, maxCol),
    };

    if (extend) {
      const anchor = sel
        ? { row: sel.r0, col: sel.c0 }
        : (lastSelectedCell ?? current);
      sel = { r0: anchor.row, c0: anchor.col, r1: next.row, c1: next.col };
    } else {
      sel = { r0: next.row, c0: next.col, r1: next.row, c1: next.col };
      lastSelectedCell = next;
    }

    ensureRowVisible(next.row);
    ensureColVisible(next.col);
    scheduleRender();
  }

  // ─── Hit-testing ─────────────────────────────────────────────────────────────
  function getCellFromMouse(e: MouseEvent): { row: number; col: number } | null {
    if (!canvas) return null;
    const rect = canvas.getBoundingClientRect();
    const mx   = e.clientX - rect.left;
    const my   = e.clientY - rect.top;

    // Ignore clicks in the row-number column
    if (mx < rowNumWidth) return null;

    const rowInView = Math.floor((my - yOffset) / rowHeight);
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

  // ─── Row-number hit testing ────────────────────────────────────────────────────
  function getRowFromMouse(e: MouseEvent): number {
    if (!canvas) return -1;
    const rect = canvas.getBoundingClientRect();
    const mx   = e.clientX - rect.left;
    const my   = e.clientY - rect.top;
    if (mx >= rowNumWidth) return -1;
    const rowInView = Math.floor((my - yOffset) / rowHeight);
    const absRow    = startRow + rowInView;
    return absRow >= 0 && absRow < totalRows ? absRow : -1;
  }

  function handleRowNumberClick(row: number, e: MouseEvent) {
    if (!result?.columns?.length) return;

    const isToggle = e.ctrlKey || e.metaKey;
    const isRange = e.shiftKey;
    const nextRows = new Set(selectedRows);

    if (isRange) {
      const anchor = rowSelectionAnchor ?? lastSelectedCell?.row ?? row;
      const lo = Math.min(anchor, row);
      const hi = Math.max(anchor, row);
      if (!isToggle) nextRows.clear();
      for (let r = lo; r <= hi; r++) nextRows.add(r);
      rowSelectionAnchor = anchor;
    } else if (isToggle) {
      if (nextRows.has(row)) nextRows.delete(row);
      else nextRows.add(row);
      rowSelectionAnchor = row;
    } else {
      nextRows.clear();
      nextRows.add(row);
      rowSelectionAnchor = row;
    }

    selectedRows = nextRows;
    sel = null;
    const lastCol = result.columns.length - 1;
    lastSelectedCell = { row, col: lastCol };
    scheduleRender();
  }

  // ─── Edge-scroll helpers ──────────────────────────────────────────────────────
  function getEdgeScrollSpeed(pos: number, size: number): number {
    if (pos < EDGE_ZONE) return -(1 - pos / EDGE_ZONE) * MAX_EDGE_SPEED;
    if (pos > size - EDGE_ZONE) return (1 - (size - pos) / EDGE_ZONE) * MAX_EDGE_SPEED;
    return 0;
  }

  function edgeScrollLoop() {
    if (!isSelecting || !scrollContainer || !lastDragMouseEvent || !canvas || !selAnchor) {
      edgeScrollRaf = 0;
      return;
    }
    const contRect   = scrollContainer.getBoundingClientRect();
    const canvasRect = canvas.getBoundingClientRect();
    const mx = lastDragMouseEvent.clientX - contRect.left;
    const my = lastDragMouseEvent.clientY - contRect.top;
    const vx = getEdgeScrollSpeed(mx, contRect.width);
    const vy = getEdgeScrollSpeed(my, contRect.height);

    if (vx !== 0 || vy !== 0) {
      const newSL = clamp(scrollContainer.scrollLeft + vx, 0, Math.max(0, totalContentWidth - containerWidth));
      const newST = clamp(scrollContainer.scrollTop  + vy, 0, Math.max(0, virtualHeight   - containerHeight));
      scrollContainer.scrollLeft = newSL;
      scrollContainer.scrollTop  = newST;
      scrollLeft = newSL;
      scrollTop  = newST;

      // Recompute virtual row position at new scrollTop (mirrors the reactive block)
      let curSR: number, curYO: number;
      if (useScaledScroll) {
        const maxSc = Math.max(0, virtualHeight - containerHeight);
        if (maxSc > 0) {
          const ratio    = newST / maxSc;
          const maxStart = Math.max(0, totalRows - containerHeight / rowHeight);
          const exactRow = ratio * maxStart;
          curSR = Math.floor(exactRow);
          curYO = -((exactRow - curSR) * rowHeight);
        } else { curSR = 0; curYO = 0; }
      } else {
        curSR = Math.floor(newST / rowHeight);
        curYO = -(newST % rowHeight);
      }

      const canvasX = lastDragMouseEvent.clientX - canvasRect.left;
      const canvasY = lastDragMouseEvent.clientY - canvasRect.top;
      if (canvasX >= rowNumWidth) {
        const rowInView = Math.floor((canvasY - curYO) / rowHeight);
        const absRow    = curSR + rowInView;
        if (absRow >= 0 && absRow < totalRows) {
          const xInContent = canvasX - rowNumWidth + newSL;
          if (xInContent >= 0) {
            let cumW = 0;
            for (let c = 0; c < colWidths.length; c++) {
              cumW += colWidths[c];
              if (xInContent < cumW) {
                if (!sel || absRow !== sel.r1 || c !== sel.c1) {
                  sel = { r0: selAnchor.row, c0: selAnchor.col, r1: absRow, c1: c };
                }
                break;
              }
            }
          }
        }
      }
      scheduleRender();
    }
    edgeScrollRaf = requestAnimationFrame(edgeScrollLoop);
  }

  function onWindowSelectionMouseMove(e: MouseEvent) {
    if (!isSelecting) return;
    lastDragMouseEvent = e;
    if (selAnchor) {
      const hit = getCellFromMouse(e);
      if (hit && sel && (hit.row !== sel.r1 || hit.col !== sel.c1)) {
        sel = { r0: selAnchor.row, c0: selAnchor.col, r1: hit.row, c1: hit.col };
        scheduleRender();
      }
    }
    if (!edgeScrollRaf) {
      edgeScrollRaf = requestAnimationFrame(edgeScrollLoop);
    }
  }

  // ─── Canvas mouse: selection ──────────────────────────────────────────────────
  function onCanvasMouseDown(e: MouseEvent) {
    if (e.button !== 0) return;
    gridWrap?.focus();

    const rowHit = getRowFromMouse(e);
    if (rowHit >= 0) {
      handleRowNumberClick(rowHit, e);
      return;
    }

    const hit = getCellFromMouse(e);
    if (!hit) {
      sel = null;
      selectedRows = new Set();
      rowSelectionAnchor = null;
      lastSelectedCell = null;
      scheduleRender();
      return;
    }

    const anchor = e.shiftKey && lastSelectedCell ? lastSelectedCell : hit;
    isSelecting = true;
    selAnchor   = anchor;
    sel         = { r0: anchor.row, c0: anchor.col, r1: hit.row, c1: hit.col };
    selectedRows = new Set();
    rowSelectionAnchor = null;
    lastSelectedCell = hit;
    scheduleRender();

    window.addEventListener('mouseup',   onWindowSelectionMouseUp);
    window.addEventListener('mousemove', onWindowSelectionMouseMove);
  }

  function onCanvasMouseMove(e: MouseEvent) {
    if (!canvas) return;
    const rect      = canvas.getBoundingClientRect();
    const mx        = e.clientX - rect.left;
    const my        = e.clientY - rect.top;
    const rowInView = Math.floor((my - yOffset) / rowHeight);
    const absRow    = startRow + rowInView;
    const next      = absRow >= 0 && absRow < totalRows ? absRow : -1;
    if (next !== hoveredRow) hoveredRow = next;
    canvas.style.cursor = mx < rowNumWidth ? 'pointer' : 'cell';
  }

  function onCanvasMouseLeave() {
    if (hoveredRow !== -1) hoveredRow = -1;
    if (canvas) canvas.style.cursor = '';
  }

  function onWindowSelectionMouseUp() {
    if (sel) {
      lastSelectedCell = { row: sel.r1, col: sel.c1 };
    }
    isSelecting = false;
    selAnchor   = null;
    lastDragMouseEvent = null;
    if (edgeScrollRaf) {
      cancelAnimationFrame(edgeScrollRaf);
      edgeScrollRaf = 0;
    }
    window.removeEventListener('mouseup',   onWindowSelectionMouseUp);
    window.removeEventListener('mousemove', onWindowSelectionMouseMove);
  }

  // Double-click: open edit overlay if editable, otherwise copy single cell value
  function onCanvasDblClick(e: MouseEvent) {
    const hit = getCellFromMouse(e);
    if (!hit) return;

    const rowDataIdx = sortIndex && hit.row < sortIndex.length ? sortIndex[hit.row] : hit.row;
    const row        = rows[rowDataIdx];
    if (!row) return;

    // Edit mode: open inline editor
    if (editInfo && editInfo.primaryKeyCols.length > 0 && canvas) {
      const existingEdit = pendingEdits[String(rowDataIdx)]?.[String(hit.col)];
      const originalVal = row[hit.col];
      const currentVal = existingEdit !== undefined ? existingEdit : originalVal;
      const valStr = currentVal === null ? '' : String(currentVal);

      const rect = canvas.getBoundingClientRect();
      let cx = rowNumWidth - scrollLeft;
      for (let i = 0; i < hit.col; i++) cx += colWidths[i] ?? 0;
      const cy = yOffset + (hit.row - startRow) * rowHeight;
      const cw = colWidths[hit.col] ?? 100;

      editOverlay = {
        row: hit.row,
        col: hit.col,
        rowDataIdx,
        value: valStr,
        x: rect.left + cx,
        y: rect.top + cy,
        w: cw,
      };

      sel = { r0: hit.row, c0: hit.col, r1: hit.row, c1: hit.col };
      lastSelectedCell = hit;
      scheduleRender();

      setTimeout(() => editInput?.focus(), 0);
      return;
    }

    // Default: copy cell value to clipboard
    const val  = row[hit.col];
  const text = formatValueForClipboard(val, result?.columnTypes?.[hit.col], 'NULL');
    navigator.clipboard.writeText(text).catch(() => {});

    sel = { r0: hit.row, c0: hit.col, r1: hit.row, c1: hit.col };
    lastSelectedCell = hit;
    scheduleRender();
  }

  function commitEdit() {
    if (!editOverlay) return;
    const { rowDataIdx, col, value } = editOverlay;
    editOverlay = null;

    const tab = currentSqlTab;
    const currentPending = tab?.pendingEdits ?? {};
    const originalVal = rows[rowDataIdx]?.[col] ?? null;
    const originalStr = originalVal === null ? '' : String(originalVal);

    const existingRowEdits = currentPending[String(rowDataIdx)] ?? {};

    if (value === originalStr) {
      // Value unchanged — remove any existing edit for this cell
      const { [String(col)]: _removed, ...restCols } = existingRowEdits;
      const newPending = { ...currentPending };
      if (Object.keys(restCols).length === 0) {
        const { [String(rowDataIdx)]: _removedRow, ...restRows } = newPending;
        tabs.updateTab(tabId, { pendingEdits: restRows });
      } else {
        tabs.updateTab(tabId, { pendingEdits: { ...newPending, [String(rowDataIdx)]: restCols } });
      }
    } else {
      tabs.updateTab(tabId, {
        pendingEdits: {
          ...currentPending,
          [String(rowDataIdx)]: { ...existingRowEdits, [String(col)]: value },
        },
      });
    }

    scheduleRender();
  }

  function onEditKeyDown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      commitEdit();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      editOverlay = null;
      scheduleRender();
    } else if (e.key === 'Tab') {
      e.preventDefault();
      commitEdit();
    }
  }

  // ─── Keyboard: copy selection / clear / select-all ───────────────────────────
  function onGridKeyDown(e: KeyboardEvent) {
    if (!result?.columns?.length || totalRows <= 0) return;

    const extend = e.shiftKey;
    const pageRows = getPageRowStep();
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      moveSelectionBy(-1, 0, extend);
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      moveSelectionBy(1, 0, extend);
      return;
    }
    if (e.key === 'ArrowLeft') {
      e.preventDefault();
      moveSelectionBy(0, -1, extend);
      return;
    }
    if (e.key === 'ArrowRight') {
      e.preventDefault();
      moveSelectionBy(0, 1, extend);
      return;
    }
    if (e.key === 'PageUp') {
      e.preventDefault();
      moveSelectionBy(-pageRows, 0, extend);
      return;
    }
    if (e.key === 'PageDown') {
      e.preventDefault();
      moveSelectionBy(pageRows, 0, extend);
      return;
    }

    if ((e.ctrlKey || e.metaKey) && e.key === 'c') {
      e.preventDefault();
      copySelection();
    }
    if ((e.ctrlKey || e.metaKey) && e.key === 'a') {
      // Only activate select-all when the grid already has a selection, so we
      // don't steal Ctrl/Cmd+A from other contexts (SQL editor, etc.).
      if ((sel !== null || selectedRows.size > 0) && rows.length > 0 && result?.columns?.length) {
        e.preventDefault();
        sel = { r0: 0, c0: 0, r1: totalRows - 1, c1: result.columns.length - 1 };
        selectedRows = new Set();
        rowSelectionAnchor = 0;
        lastSelectedCell = { row: totalRows - 1, col: result.columns.length - 1 };
        scheduleRender();
      }
    }
    if (e.key === 'Escape') {
      sel = null;
      selectedRows = new Set();
      rowSelectionAnchor = null;
      lastSelectedCell = null;
      scheduleRender();
    }
  }

  function copySelection() {
    if (!result) return;

    if (selectedRows.size > 0) {
      const lines: string[] = [];
      const sortedRows = Array.from(selectedRows).sort((a, b) => a - b);
      for (const r of sortedRows) {
        const rowDataIdx = sortIndex && r < sortIndex.length ? sortIndex[r] : r;
        const row = rows[rowDataIdx];
        if (!row) continue;
        const cells: string[] = [];
        for (let c = 0; c < result.columns.length; c++) {
          const v = c < row.length ? row[c] : null;
          cells.push(escapeTsvCell(formatValueForClipboard(v, result.columnTypes?.[c], '')));
        }
        lines.push(cells.join('\t'));
      }
      if (lines.length > 0) {
        navigator.clipboard.writeText(lines.join('\n')).catch(() => {});
      }
      return;
    }

    if (!sel) return;

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
        cells.push(escapeTsvCell(formatValueForClipboard(v, result.columnTypes?.[c], '')));
      }
      lines.push(cells.join('\t'));
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
    // Reset here so the very next header click triggers a sort normally.
    // The resize handle's on:click|stopPropagation already prevents any drag
    // mouseup from bubbling into the header's click handler, so keeping
    // didResize=true after stopResize only caused the first post-resize
    // header click to be swallowed unnecessarily.
    didResize = false;
  }

  // ─── Auto-fit column on resize-handle double-click ───────────────────────────
  // Applies the widest measured header or cell width for this result.
  function autoFitColumn(e: MouseEvent, idx: number) {
    e.preventDefault();
    e.stopPropagation();
    if (colTextMeasurements[idx]) {
      colWidths[idx] = columnTextWidth(idx);
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
    selectedRows = new Set();
    rowSelectionAnchor = null;
    lastSelectedCell = null;
    editOverlay = null;
    if (tabId) tabs.updateTab(tabId, { sortCol: -1, sortDirection: 'asc', pendingEdits: {} });
  }

  // ─── Click-outside: clear selection ──────────────────────────────────────────
  function onDocumentMouseDown(e: MouseEvent) {
    if (sel === null && selectedRows.size === 0) return;
    if (gridWrap && !gridWrap.contains(e.target as Node)) {
      sel = null;
      selectedRows = new Set();
      rowSelectionAnchor = null;
      lastSelectedCell = null;
      scheduleRender();
    }
  }

  // ─── Lifecycle ────────────────────────────────────────────────────────────────
  onMount(() => {
    resolveColors();
    scheduleRender();
    document.addEventListener('mousedown', onDocumentMouseDown);
  });

  onDestroy(() => {
    if (rafId) cancelAnimationFrame(rafId);
    if (edgeScrollRaf) cancelAnimationFrame(edgeScrollRaf);
    window.removeEventListener('mouseup',   onWindowSelectionMouseUp);
    window.removeEventListener('mousemove', onWindowSelectionMouseMove);
    window.removeEventListener('mousemove', onResize);
    window.removeEventListener('mouseup',   stopResize);
    document.removeEventListener('mousedown', onDocumentMouseDown);
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
    bind:this={gridWrap}
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
              on:mouseenter={e => onHeaderMouseEnter(e, i)}
              on:mouseleave={onHeaderMouseLeave}
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
                on:keydown|stopPropagation={() => {}}
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

  {#if tooltipCol !== null}
    <div
      class="col-tooltip"
      style="left:{tooltipX}px; top:{tooltipY}px"
      role="tooltip"
    >
      {#if result?.columnTypes?.[tooltipCol]}
        <span class="col-tooltip-type">{result.columnTypes[tooltipCol]}</span>
      {/if}
      <span class="col-tooltip-len">max length: {colTextMeasurements[tooltipCol]?.maxLength ?? 0}</span>
    </div>
  {/if}

  {#if editOverlay}
    <!-- svelte-ignore a11y-autofocus -->
    <input
      bind:this={editInput}
      type="text"
      class="cell-edit-input"
      style="left:{editOverlay.x}px; top:{editOverlay.y}px; width:{editOverlay.w}px; height:{rowHeight}px;"
      bind:value={editOverlay.value}
      on:keydown={onEditKeyDown}
      on:blur={commitEdit}
    />
  {/if}
{/if}

<style>
  .empty {
    display: flex; align-items: center; justify-content: center;
    height: 100%; color: var(--text-muted); font-size: calc(13px * var(--app-font-scale));
  }
  .empty.error { color: var(--error); }

  .grid-wrap {
    display: flex; flex-direction: column;
    height: 100%; min-height: 0; overflow: hidden;
    outline: none; /* suppress browser focus ring on the container */
  }

  .grid-header {
    display: flex; align-items: stretch;
    background: var(--bg-panel);
    border-bottom: 2px solid var(--border);
    flex-shrink: 0;
    font-size: calc(12px * var(--app-font-scale)); font-weight: 600; color: var(--text-muted);
  }
  .row-num-header {
    flex-shrink: 0;
    display: flex; align-items: center; justify-content: center;
    padding: 6px 8px; font-size: calc(11px * var(--app-font-scale)); color: var(--text-muted);
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
  .sort-icon  { opacity: 0.7; font-size: calc(10px * var(--app-font-scale)); flex-shrink: 0; }

  /* Wider hit-area (6 px) makes double-click easier to land precisely */
  .resize-handle {
    position: absolute; right: 0; top: 0; bottom: 0; width: 6px;
    cursor: col-resize; z-index: 1;
  }
  .resize-handle:hover { background: var(--accent); opacity: 0.5; }

  .col-tooltip {
    position: fixed;
    z-index: 100;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 6px 10px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    pointer-events: none;
    box-shadow: 0 4px 12px rgba(0,0,0,0.4);
  }
  .col-tooltip-type {
    font-size: calc(11px * var(--app-font-scale));
    font-weight: 600;
    color: var(--accent, #4682ff);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .col-tooltip-len {
    font-size: calc(11px * var(--app-font-scale));
    color: var(--text-muted);
  }

  .grid-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    scrollbar-width: thin;
    scrollbar-color: rgba(236, 240, 248, 0.35) rgba(255, 255, 255, 0.2);
  }
  /* Keep result-grid scrollbars readable on newer macOS overlay styles. */
  .grid-body::-webkit-scrollbar {
    width: 12px;
    height: 12px;
  }
  .grid-body::-webkit-scrollbar-track {
    background: rgba(255, 255, 255, 0.2);
  }
  .grid-body::-webkit-scrollbar-thumb {
    background: rgba(236, 240, 248, 0.35);
    border: 3px solid rgba(255, 255, 255, 0.2);
    border-radius: 999px;
  }
  .grid-body::-webkit-scrollbar-thumb:hover {
    background: rgba(246, 249, 255, 0.68);
  }
  /* Cell cursor to hint that data is selectable */
  .grid-body canvas { cursor: cell; display: block; }

  .cell-edit-input {
    position: fixed;
    z-index: 200;
    box-sizing: border-box;
    padding: 0 10px;
    background: var(--bg-panel, #1a1a24);
    color: var(--text, #e2e2f0);
    border: 2px solid rgba(255,200,50,0.9);
    border-radius: 0;
    font: calc(12px * var(--app-font-scale)) -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    outline: none;
  }
  .cell-edit-input:focus {
    border-color: rgba(255,200,50,1);
    background: rgba(255,200,50,0.06);
  }

  .sr-only {
    position: absolute; width: 1px; height: 1px;
    overflow: hidden; clip: rect(0,0,0,0);
    padding: 0; margin: -1px; border: 0;
  }
</style>
