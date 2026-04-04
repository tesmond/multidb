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
    // First header is the row-number column, then one per column
    expect(headers.length).toBeGreaterThanOrEqual(mockResult.columns.length);
    const headerTexts = headers.map((h: HTMLElement) => h.textContent?.trim());
    expect(headerTexts).toContain('id');
    expect(headerTexts).toContain('name');
    expect(headerTexts).toContain('age');
  });

  it('renders row count in status bar', () => {
    const { container } = render(ResultsGrid, { props: { result: mockResult } });
    const text = container.textContent ?? '';
    expect(text).toContain('3');
  });

  it('handles empty rows gracefully', () => {
    const empty = { ...mockResult, rows: [] };
    const { container } = render(ResultsGrid, { props: { result: empty } });
    // Grid renders headers but no data rows
    expect(container.querySelector('.grid-header')).toBeTruthy();
  });
});
