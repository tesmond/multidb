import { describe, expect, it } from 'vitest';
import { CompletionContext, type Completion, type CompletionResult } from '@codemirror/autocomplete';
import { MySQL, sql as sqlLanguage } from '@codemirror/lang-sql';
import { EditorState } from '@codemirror/state';
import { findSqlSemanticDiagnostics, makeSmartCompletionSource, type DbSchema } from './sqlComplete';

const mysqlSchema: DbSchema = {
  driver: 'mysql',
  schemas: [
    {
      name: 'app',
      tables: [
        {
          name: 'users',
          columns: [
            { name: 'id' },
            { name: 'username' },
            { name: 'data_length' },
          ],
        },
        {
          name: 'orders',
          columns: [
            { name: 'order_id' },
            { name: 'user_id' },
          ],
        },
      ],
    },
  ],
};

function completionsAt(query: string, cursor = query.length, explicit = true): CompletionResult {
  const state = EditorState.create({
    doc: query,
    extensions: [sqlLanguage({ dialect: MySQL })],
  });
  const source = makeSmartCompletionSource(mysqlSchema);
  const result = source(new CompletionContext(state, cursor, explicit));

  expect(result).not.toBeNull();
  expect(result).not.toBeInstanceOf(Promise);
  return result as CompletionResult;
}

function option(result: CompletionResult, label: string): Completion {
  const completion = result.options.find((candidate) => candidate.label === label);
  expect(completion, `Expected completion ${label}`).toBeDefined();
  return completion!;
}

describe('SQL completion ranking', () => {
  it.each(['SELECT ', 'SELECT * FROM users WHERE '])(
    'offers expression completions automatically after %s',
    (query) => {
      const result = completionsAt(query, query.length, false);

      expect(option(result, 'COUNT').boost).toBeGreaterThan(option(result, 'users').boost!);
      expect(option(result, 'username').boost).toBeGreaterThan(option(result, 'users').boost!);
    },
  );

  it('ranks matching functions and in-scope columns above tables in SELECT expressions', () => {
    const functionResult = completionsAt('SELECT co');
    expect(option(functionResult, 'COUNT').type).toBe('function');

    const sql = 'SELECT  FROM users';
    const result = completionsAt(sql, 'SELECT '.length);
    expect(option(result, 'COUNT').boost).toBeGreaterThan(option(result, 'users').boost!);
    expect(option(result, 'username').boost).toBeGreaterThan(option(result, 'users').boost!);
  });

  it('ranks columns from referenced tables above unrelated columns in WHERE expressions', () => {
    const result = completionsAt('SELECT * FROM users WHERE ');

    expect(option(result, 'username').boost).toBeGreaterThan(option(result, 'order_id').boost!);
    expect(option(result, 'username').boost).toBeGreaterThan(option(result, 'users').boost!);
  });

  it.each(['FROM', 'JOIN'])('keeps tables above matching columns after %s with a partial name', (clause) => {
    const result = completionsAt(`SELECT * FROM orders ${clause} us`);

    expect(option(result, 'users').boost).toBeGreaterThanOrEqual(90);
    expect(result.options.some((candidate) => candidate.label === 'username')).toBe(false);
  });
});

describe('SQL semantic diagnostics', () => {
  it('allows MySQL information_schema table size queries', () => {
    const sql = `
SELECT
    table_schema AS database_name,
    table_name,
    ROUND(data_length / 1024 / 1024, 2) AS data_mb,
    ROUND(index_length / 1024 / 1024, 2) AS index_mb,
    ROUND((data_length + index_length) / 1024 / 1024, 2) AS total_mb
FROM information_schema.tables
ORDER BY total_mb DESC;
`;

    expect(findSqlSemanticDiagnostics(sql, mysqlSchema)).toEqual([]);
  });

  it('does not flag SQL function names as unknown columns', () => {
    const sql = 'SELECT ROUND(data_length / 1024, 2) AS data_mb FROM users';

    expect(findSqlSemanticDiagnostics(sql, mysqlSchema)).toEqual([]);
  });

  it('reports unknown bare selected columns', () => {
    const diagnostics = findSqlSemanticDiagnostics('SELECT missing_column FROM users', mysqlSchema);

    expect(diagnostics.map((diagnostic) => diagnostic.message)).toContain(
      'Unknown column missing_column',
    );
  });

  it('reports unknown columns inside SQL function expressions', () => {
    const sql = 'SELECT ROUND(missing_column / 1024, 2) AS data_mb FROM users';
    const diagnostics = findSqlSemanticDiagnostics(sql, mysqlSchema);

    expect(diagnostics.map((diagnostic) => diagnostic.message)).toContain(
      'Unknown column missing_column',
    );
  });

  it('reports unknown columns on known system tables', () => {
    const sql = 'SELECT table_schemo FROM information_schema.tables';
    const diagnostics = findSqlSemanticDiagnostics(sql, mysqlSchema);

    expect(diagnostics.map((diagnostic) => diagnostic.message)).toContain(
      'Unknown column table_schemo',
    );
  });

  it('still reports unknown application tables', () => {
    const diagnostics = findSqlSemanticDiagnostics('SELECT id FROM missing_table', mysqlSchema);

    expect(diagnostics.map((diagnostic) => diagnostic.message)).toContain(
      'Unknown table missing_table',
    );
  });
});
