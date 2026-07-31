import { describe, expect, it } from 'vitest';
import {
  escapeTsvCell,
  formatValueForClipboard,
  isJsonColumnType,
  prettyPrintJsonText,
} from './resultClipboard';

describe('resultClipboard helpers', () => {
  it('detects JSON column types', () => {
    expect(isJsonColumnType('JSON')).toBe(true);
    expect(isJsonColumnType('jsonb')).toBe(true);
    expect(isJsonColumnType('TEXT')).toBe(false);
  });

  it('pretty prints valid JSON text', () => {
    expect(prettyPrintJsonText('{"a":1,"b":{"c":true}}')).toBe(
      '{\n  "a": 1,\n  "b": {\n    "c": true\n  }\n}',
    );
  });

  it('leaves invalid JSON text unchanged', () => {
    expect(prettyPrintJsonText('{oops')).toBe('{oops');
  });

  it('formats JSON cells for clipboard with pretty printing', () => {
    expect(formatValueForClipboard('{"a":1}', 'JSON')).toBe('{\n  "a": 1\n}');
  });

  it('formats null cells with configured fallback', () => {
    expect(formatValueForClipboard(null, 'JSON', 'NULL')).toBe('NULL');
  });

  it('quotes multiline TSV cells safely', () => {
    expect(escapeTsvCell('{\n  "a": 1\n}')).toBe('"{\n  ""a"": 1\n}"');
  });
});
