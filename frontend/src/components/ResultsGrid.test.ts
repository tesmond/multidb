import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/svelte';
import ResultsGrid from './ResultsGrid.svelte';

const mockResult = {
  columns: ['id', 'name', 'age'],
  rows: [
    [1, 'Alice', 30],
    [2, 'Bob', 25],
    [3, 'Charlie', 35],
  ],
  rowsAffected: 0,
  duration: 12,
  error: '',
};

describe('ResultsGrid', () => {
  it('renders no-result message when result is null', () => {
    const { getByText } = render(ResultsGrid, { props: { result: null } });
    expect(getByText(/run a query/i)).toBeTruthy();
  });

  it('renders an error message when result has an error', () => {
    const errorResult = { ...mockResult, error: 'Syntax error near LINE 1' };
    const { getByText } = render(ResultsGrid, { props: { result: errorResult } });
    expect(getByText(/Syntax error/i)).toBeTruthy();
  });

  it('renders column headers', () => {
    const { getAllByRole } = render(ResultsGrid, { props: { result: mockResult } });
    const headers = getAllByRole('columnheader');
    // Row-number header (#) plus one per data column
    expect(headers.length).toBe(mockResult.columns.length + 1);
    const headerTexts = headers.map((h: HTMLElement) => h.textContent?.trim());
    expect(headerTexts).toContain('id');
    expect(headerTexts).toContain('name');
    expect(headerTexts).toContain('age');
  });

  it('exposes row count via aria-rowcount', () => {
    const { container } = render(ResultsGrid, { props: { result: mockResult } });
    const grid = container.querySelector('[role="grid"]');
    expect(grid?.getAttribute('aria-rowcount')).toBe('3');
  });

  it('includes row count in accessible text', () => {
    const { container } = render(ResultsGrid, { props: { result: mockResult } });
    const text = container.textContent ?? '';
    expect(text).toContain('3 rows');
  });

  it('handles empty rows gracefully', () => {
    const empty = { ...mockResult, rows: [] };
    const { container } = render(ResultsGrid, { props: { result: empty } });
    // Grid renders headers but no data rows
    expect(container.querySelector('.grid-header')).toBeTruthy();
  });
});
