import { describe, expect, it } from 'vitest';
import { calculateAutoFitColumnWidth } from './columnSizing';

const metrics = {
  cellPaddingX: 10,
  fontScale: 1,
};

describe('calculateAutoFitColumnWidth', () => {
  it('uses the measured header width when the header is wider than every cell', () => {
    expect(calculateAutoFitColumnWidth(12, 140, metrics)).toBe(188);
  });

  it('leaves extra room around the measured widest cell text', () => {
    expect(calculateAutoFitColumnWidth(120, 10, metrics)).toBe(152);
  });
});
