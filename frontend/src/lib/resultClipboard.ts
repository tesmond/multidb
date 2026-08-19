export function isJsonColumnType(columnType?: string | null): boolean {
  return /\bjsonb?\b/i.test(columnType ?? '');
}

export function prettyPrintJsonText(text: string): string {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

export function formatValueForClipboard(
  value: unknown,
  columnType?: string | null,
  nullText = '',
): string {
  if (value === null || value === undefined) {
    return nullText;
  }

  const text = String(value);
  if (isJsonColumnType(columnType)) {
    return prettyPrintJsonText(text);
  }

  return text;
}

export function escapeTsvCell(text: string): string {
  if (!/[\t\n\r\"]/.test(text)) {
    return text;
  }
  return `"${text.replace(/"/g, '""')}"`;
}
