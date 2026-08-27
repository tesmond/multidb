export interface ColumnWidthMetrics {
  cellPaddingX: number;
  fontScale: number;
}

const CELL_FIT_BUFFER = 12;
const HEADER_AFFORDANCE_WIDTH = 28;

export function calculateAutoFitColumnWidth(
  maxCellTextWidth: number,
  headerTextWidth: number,
  metrics: ColumnWidthMetrics,
): number {
  const { cellPaddingX, fontScale } = metrics;
  const cellWidth =
    maxCellTextWidth * fontScale
    + cellPaddingX * 2
    + CELL_FIT_BUFFER * fontScale;
  const headerWidth =
    headerTextWidth * fontScale
    + cellPaddingX * 2
    + HEADER_AFFORDANCE_WIDTH * fontScale;

  return Math.min(
    Math.max(cellWidth, headerWidth, 50 * fontScale),
    800 * fontScale,
  );
}
