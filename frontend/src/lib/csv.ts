// Shared CSV export utilities used by ResultsGrid and StatusBar.
//
// On Wails/macOS WKWebView, blob URL downloads via simulated anchor clicks do
// not work.  All save operations are therefore routed through the native Wails
// SaveCSV backend call which opens a system save-file dialog and writes the
// file from Go.

import { SaveCSV } from "../../wailsjs/go/main/App";

export function escapeCSV(value: any): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "object") {
    try {
      return csvSafeString(JSON.stringify(value));
    } catch {
      return csvSafeString(String(value));
    }
  }
  return csvSafeString(String(value));
}

function csvSafeString(str: string): string {
  if (
    str.includes(",") ||
    str.includes('"') ||
    str.includes("\n") ||
    str.includes("\r")
  ) {
    return '"' + str.replace(/"/g, '""') + '"';
  }
  return str;
}

export interface BuildCSVOptions {
  /** Optional sort index mapping visible row order to rows array indices. */
  sortIndex?: number[] | null;
  /** Line terminator – default '\n' */
  lineTerminator?: string;
}

/**
 * Build a CSV string from columns and rows.
 */
export function buildCSV(
  columns: string[],
  rows: any[][],
  options: BuildCSVOptions = {},
): string {
  if (!columns || columns.length === 0) return "";

  const lineTerminator = options.lineTerminator ?? "\n";
  const n = rows.length;
  const sortIndex = options.sortIndex ?? null;

  const header = columns.map((col) => escapeCSV(col)).join(",");
  const dataRows: string[] = [];

  if (sortIndex && sortIndex.length > 0) {
    for (let i = 0; i < sortIndex.length; i++) {
      const idx = sortIndex[i];
      if (idx == null || idx < 0 || idx >= n) continue;
      dataRows.push(rowToCSV(rows[idx] ?? [], columns.length));
    }
  } else {
    for (let i = 0; i < n; i++) {
      dataRows.push(rowToCSV(rows[i] ?? [], columns.length));
    }
  }

  return [header, ...dataRows].join(lineTerminator);
}

function rowToCSV(row: any[], expectedCols: number): string {
  const out: string[] = new Array(expectedCols);
  for (let c = 0; c < expectedCols; c++) {
    out[c] = escapeCSV(c < row.length ? row[c] : null);
  }
  return out.join(",");
}

/**
 * Open the native OS save dialog (via the Wails Go backend) and write the CSV.
 * This works correctly on macOS where WKWebView does not support blob downloads.
 */
export async function saveCSVNative(
  content: string,
  filename = "query_results.csv",
): Promise<void> {
  await SaveCSV(content, filename);
}

/**
 * Build a CSV from columns + rows and save it via the native dialog.
 */
export async function exportToCSV(
  columns: string[],
  rows: any[][],
  filename = "query_results.csv",
  options: BuildCSVOptions = {},
): Promise<void> {
  if (!columns || !Array.isArray(rows)) return;
  const csv = buildCSV(columns, rows, options);
  if (!csv) return;
  await saveCSVNative(csv, filename);
}

/**
 * Convenience wrapper that accepts the ExecuteResult shape from the backend.
 */
export async function exportResultToCSV(
  result: { columns?: string[]; rows?: any[][] } | null | undefined,
  filename = "query_results.csv",
): Promise<void> {
  if (!result || !result.columns || !result.rows) return;
  await exportToCSV(result.columns, result.rows, filename);
}
