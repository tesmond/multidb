import { describe, expect, it } from 'vitest';
import { findSqlSemanticDiagnostics, type DbSchema } from './sqlComplete';

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
            { name: 'data_length' },
          ],
        },
      ],
    },
  ],
};

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
