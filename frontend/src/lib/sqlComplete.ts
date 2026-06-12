/**
 * sqlComplete.ts
 *
 * Smart SQL auto-completion engine for CodeMirror 6.
 *
 * Responsibilities:
 *   1. buildSqlNamespace()        – converts the app's schema tree into the
 *                                   hierarchical SQLNamespace that
 *                                   @codemirror/lang-sql understands so that
 *                                   schema.table and table.column dot-
 *                                   completions work for free.
 *
 *   2. makeSmartCompletionSource() – a CodeMirror CompletionSource that adds
 *                                   the things the built-in source cannot:
 *
 *       • schema.table.col        (two-dot path)
 *       • alias.col               (FROM/JOIN alias resolution)
 *       • table.col               (unqualified table prefix)
 *       • free-standing names     (schemas, tables, columns) when no dot is
 *                                  present but the cursor is on a word
 *
 * No extra npm dependencies are needed – we re-use the @codemirror/* packages
 * already pulled in by @codemirror/lang-sql and codemirror.
 */

import type { CompletionContext, CompletionResult, Completion } from '@codemirror/autocomplete';

// ─── Public data types ────────────────────────────────────────────────────────

export interface ColInfo {
  name: string;
  type?: string;
  key?: string;       // e.g. 'PRI'
}

export interface TableInfo {
  name: string;
  columns: ColInfo[];
  isView?: boolean;
}

export interface SchemaInfo {
  name: string;       // e.g. 'public' (Postgres), 'mydb' (MySQL)
  tables: TableInfo[];
}

/**
 * Normalised schema data for one database connection.
 * Either `schemas` (Postgres / MySQL multi-schema) or `tables` + `views`
 * (SQLite / flat) will be populated.
 */
export interface DbSchema {
  driver: string;                 // 'postgres' | 'mysql' | 'sqlite'
  schemas?: SchemaInfo[];         // present for multi-schema drivers
  tables?:  TableInfo[];          // present for flat drivers
  views?:   TableInfo[];
}

export interface SqlSemanticDiagnostic {
  from: number;
  to: number;
  message: string;
}

export interface SqlSyntaxDiagnostic {
  from: number;
  to: number;
  message: string;
}

// ─── Namespace builder ────────────────────────────────────────────────────────

/**
 * Builds the SQLNamespace object consumed by @codemirror/lang-sql's
 * schemaCompletionSource.
 *
 * Structure produced:
 *
 *   Postgres / MySQL:
 *     {
 *       "public":  { "users": ["id","name"], "orders": ["id","total"] },
 *       // also exposed at top level so unqualified names complete:
 *       "users":   ["id", "name"],
 *       "orders":  ["id", "total"],
 *     }
 *
 *   SQLite (flat):
 *     {
 *       "users":  ["id", "name"],
 *       "orders": ["id", "total"],
 *     }
 */
export function buildSqlNamespace(db: DbSchema): Record<string, any> {
  const ns: Record<string, any> = {};

  if (db.schemas?.length) {
    for (const schema of db.schemas) {
      const schemaNs: Record<string, string[]> = {};

      for (const t of schema.tables) {
        const cols = t.columns.map(c => c.name);
        schemaNs[t.name] = cols;
        // Unqualified fallback – first schema wins if names clash
        if (!(t.name in ns)) ns[t.name] = cols;
      }

      ns[schema.name] = schemaNs;
    }
  } else {
    // Flat: SQLite or a single-DB connection
    const all = [...(db.tables ?? []), ...(db.views ?? [])];
    for (const t of all) {
      ns[t.name] = t.columns.map(c => c.name);
    }
  }

  return ns;
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/**
 * SQL reserved words that should never be treated as table aliases.
 */
const RESERVED = new Set([
  'WHERE','ON','SET','GROUP','ORDER','HAVING','LIMIT','OFFSET',
  'INNER','LEFT','RIGHT','OUTER','CROSS','NATURAL','FULL',
  'SELECT','FROM','JOIN','INTO','VALUES','UPDATE','DELETE',
  'INSERT','CREATE','DROP','ALTER','TABLE','INDEX','VIEW',
  'AS','BY','AND','OR','NOT','IN','IS','NULL','LIKE',
  'BETWEEN','EXISTS','CASE','WHEN','THEN','ELSE','END',
  'DISTINCT','ALL','ANY','UNION','INTERSECT','EXCEPT',
  'WITH','RECURSIVE','RETURNING','USING','LATERAL',
  'TRUE','FALSE','PRIMARY','FOREIGN','KEY','UNIQUE',
  'CONSTRAINT','DEFAULT','CHECK','REFERENCES',
]);

/** Flat map: tableName.toLowerCase() → ColInfo[] for the whole connection. */
function buildTableMap(db: DbSchema): Map<string, ColInfo[]> {
  const m = new Map<string, ColInfo[]>();

  if (db.schemas?.length) {
    for (const s of db.schemas) {
      for (const t of s.tables) {
        m.set(t.name.toLowerCase(), t.columns);
      }
    }
  } else {
    for (const t of [...(db.tables ?? []), ...(db.views ?? [])]) {
      m.set(t.name.toLowerCase(), t.columns);
    }
  }

  return m;
}

/**
 * Extracts table aliases from the full SQL text using regex scanning.
 *
 * Handles:
 *   FROM   table           alias
 *   FROM   table AS        alias
 *   JOIN   schema.table    alias
 *   JOIN   schema.table AS alias
 *   FROM  "quoted_table"   alias
 *   FROM  `backtick`       alias
 *
 * Returns Map<alias → ColInfo[]> (also includes unaliased table names so that
 * "table.col" works in the dot-completion path even without an alias).
 */
function extractAliases(sql: string, db: DbSchema): Map<string, ColInfo[]> {
  const tableMap = buildTableMap(db);
  const result   = new Map<string, ColInfo[]>();

  // Add every table/view under its own name (covers "table.column" when no
  // alias is used)
  for (const [name, cols] of tableMap) {
    result.set(name, cols);
  }

  // Match FROM / JOIN [schema.]table [AS] alias
  // The schema prefix (schema.) is optional and ignored for column lookup.
  const aliasRe =
    /\b(?:FROM|JOIN)\s+(?:[\w"'`[\]]+\s*\.\s*)?([\w"'`[\]]+)\s+(?:AS\s+)?([\w"'`[\]]+)/gi;

  let m: RegExpExecArray | null;
  while ((m = aliasRe.exec(sql)) !== null) {
    const rawTable = m[1].replace(/["`'[\]]/g, '');
    const rawAlias = m[2].replace(/["`'[\]]/g, '');

    if (RESERVED.has(rawAlias.toUpperCase())) continue;
    // If the "alias" is the same as the table name it's not really an alias,
    // but we still map it (already in the map from the loop above).
    const cols = tableMap.get(rawTable.toLowerCase());
    if (cols) {
      result.set(rawAlias,            cols);
      result.set(rawAlias.toLowerCase(), cols);
    }
  }

  // Also handle CTE names:  WITH cte_name AS ( ... )
  const cteRe = /\bWITH\s+([\w"'`]+)\s+AS\s*\(/gi;
  while ((m = cteRe.exec(sql)) !== null) {
    const cteName = m[1].replace(/["`']/g, '');
    // We don't know CTE columns ahead of time, so skip column completions
    // but at least register the CTE name so it appears in table lists.
    if (!result.has(cteName.toLowerCase())) {
      result.set(cteName.toLowerCase(), []);
    }
  }

  return result;
}

// ─── Completion item factories ────────────────────────────────────────────────

function colCompletion(c: ColInfo): Completion {
  const detail = [c.type, c.key === 'PRI' ? 'PK' : '']
    .filter(Boolean).join(' · ');
  return {
    label:  c.name,
    type:   'property',
    detail: detail || undefined,
    boost:  c.key === 'PRI' ? 12 : 10,
  };
}

function tableCompletion(t: TableInfo): Completion {
  return {
    label: t.name,
    type:  t.isView ? 'interface' : 'class',
    boost: 7,
  };
}

function schemaCompletion(name: string): Completion {
  return { label: name, type: 'namespace', boost: 5 };
}

type TableRef = {
  schema: string;
  table: string;
  columns: Set<string> | null;
  from: number;
  to: number;
};

const SYSTEM_SCHEMAS_BY_DRIVER: Record<string, Set<string>> = {
  mysql: new Set(['information_schema', 'mysql', 'performance_schema', 'sys']),
  postgres: new Set(['information_schema', 'pg_catalog']),
};

const SYSTEM_TABLE_COLUMNS_BY_DRIVER: Record<string, Record<string, string[]>> = {
  mysql: {
    'information_schema.tables': [
      'table_catalog',
      'table_schema',
      'table_name',
      'table_type',
      'engine',
      'version',
      'row_format',
      'table_rows',
      'avg_row_length',
      'data_length',
      'max_data_length',
      'index_length',
      'data_free',
      'auto_increment',
      'create_time',
      'update_time',
      'check_time',
      'table_collation',
      'checksum',
      'create_options',
      'table_comment',
    ],
  },
  postgres: {
    'information_schema.tables': [
      'table_catalog',
      'table_schema',
      'table_name',
      'table_type',
      'self_referencing_column_name',
      'reference_generation',
      'user_defined_type_catalog',
      'user_defined_type_schema',
      'user_defined_type_name',
      'is_insertable_into',
      'is_typed',
      'commit_action',
    ],
  },
};

function normalizeIdent(id: string): string {
  return id.replace(/^["'`\[]+|["'`\]]+$/g, '').toLowerCase();
}

function isSystemSchema(db: DbSchema, schemaName: string): boolean {
  return SYSTEM_SCHEMAS_BY_DRIVER[db.driver]?.has(schemaName.toLowerCase()) ?? false;
}

function getSystemTableColumns(db: DbSchema, schemaName: string, tableName: string): Set<string> | null {
  const columns = SYSTEM_TABLE_COLUMNS_BY_DRIVER[db.driver]?.[`${schemaName}.${tableName}`];
  return columns ? new Set(columns) : null;
}

function flattenDbSchema(db: DbSchema): {
  schemas: Set<string>;
  schemaTables: Map<string, Set<string>>;
  tableColumns: Map<string, Set<string>>;
} {
  const schemas = new Set<string>();
  const schemaTables = new Map<string, Set<string>>();
  const tableColumns = new Map<string, Set<string>>();

  if (db.schemas?.length) {
    for (const s of db.schemas) {
      const schemaName = s.name.toLowerCase();
      schemas.add(schemaName);
      if (!schemaTables.has(schemaName)) schemaTables.set(schemaName, new Set<string>());
      const tableSet = schemaTables.get(schemaName)!;

      for (const t of s.tables) {
        const tableName = t.name.toLowerCase();
        tableSet.add(tableName);
        if (!tableColumns.has(tableName)) {
          tableColumns.set(tableName, new Set(t.columns.map(c => c.name.toLowerCase())));
        }
      }
    }
  } else {
    for (const t of [...(db.tables ?? []), ...(db.views ?? [])]) {
      const tableName = t.name.toLowerCase();
      tableColumns.set(tableName, new Set(t.columns.map(c => c.name.toLowerCase())));
    }
  }

  return { schemas, schemaTables, tableColumns };
}

function splitSelectExpressions(selectText: string): Array<{ text: string; from: number; to: number }> {
  const parts: Array<{ text: string; from: number; to: number }> = [];
  let start = 0;
  let depth = 0;
  let quote: 'single' | 'double' | 'backtick' | null = null;

  for (let i = 0; i < selectText.length; i++) {
    const ch = selectText[i];

    if (quote) {
      if (
        (quote === 'single' && ch === "'") ||
        (quote === 'double' && ch === '"') ||
        (quote === 'backtick' && ch === '`')
      ) {
        quote = null;
      }
      continue;
    }

    if (ch === "'") quote = 'single';
    else if (ch === '"') quote = 'double';
    else if (ch === '`') quote = 'backtick';
    else if (ch === '(') depth++;
    else if (ch === ')' && depth > 0) depth--;
    else if (ch === ',' && depth === 0) {
      const text = selectText.slice(start, i).trim();
      if (text) {
        const leading = selectText.slice(start, i).search(/\S/);
        const from = start + (leading >= 0 ? leading : 0);
        parts.push({ text, from, to: i });
      }
      start = i + 1;
    }
  }

  const tailRaw = selectText.slice(start);
  const tail = tailRaw.trim();
  if (tail) {
    const leading = tailRaw.search(/\S/);
    const from = start + (leading >= 0 ? leading : 0);
    parts.push({ text: tail, from, to: selectText.length });
  }

  return parts;
}

export function findSqlCommonSyntaxDiagnostics(sqlText: string): SqlSyntaxDiagnostic[] {
  const diagnostics: SqlSyntaxDiagnostic[] = [];

  // Detect: SELECT ..., FROM ... (trailing comma before FROM)
  const selectMatch = /\bSELECT\b([\s\S]*?)\bFROM\b/gi;
  let m: RegExpExecArray | null;
  while ((m = selectMatch.exec(sqlText)) !== null) {
    const selectBody = m[1] ?? '';
    const selectBodyStart = (m.index ?? 0) + m[0].indexOf(selectBody);

    let depth = 0;
    let quote: 'single' | 'double' | 'backtick' | null = null;

    for (let i = 0; i < selectBody.length; i++) {
      const ch = selectBody[i];

      if (quote) {
        if (
          (quote === 'single' && ch === "'") ||
          (quote === 'double' && ch === '"') ||
          (quote === 'backtick' && ch === '`')
        ) {
          quote = null;
        }
        continue;
      }

      if (ch === "'") quote = 'single';
      else if (ch === '"') quote = 'double';
      else if (ch === '`') quote = 'backtick';
      else if (ch === '(') depth++;
      else if (ch === ')' && depth > 0) depth--;
      else if (ch === ',' && depth === 0) {
        const tail = selectBody.slice(i + 1);
        if (!/\S/.test(tail)) {
          diagnostics.push({
            from: selectBodyStart + i,
            to: selectBodyStart + i + 1,
            message: 'Trailing comma in SELECT list',
          });
          continue;
        }

        const nextToken = /\S+/.exec(tail);
        if (!nextToken) continue;
        const token = nextToken[0].toUpperCase();
        if (token === 'FROM') {
          diagnostics.push({
            from: selectBodyStart + i,
            to: selectBodyStart + i + 1,
            message: 'Trailing comma in SELECT list',
          });
        }
      }
    }
  }

  return diagnostics;
}

function resolveTableReferences(sqlText: string, db: DbSchema, diagnostics: SqlSemanticDiagnostic[]): TableRef[] {
  const refs: TableRef[] = [];
  const { schemas, schemaTables, tableColumns } = flattenDbSchema(db);
  const tableRefRe = /\b(?:FROM|JOIN)\s+([A-Za-z_][\w$]*)(?:\s*\.\s*([A-Za-z_][\w$]*))?/gi;

  let m: RegExpExecArray | null;
  while ((m = tableRefRe.exec(sqlText)) !== null) {
    const first = m[1];
    const second = m[2];
    const firstLower = normalizeIdent(first);
    const secondLower = second ? normalizeIdent(second) : '';

    const rawMatch = m[0];
    const firstOffsetInMatch = rawMatch.search(/[A-Za-z_][\w$]*/);
    const from = (m.index ?? 0) + Math.max(0, firstOffsetInMatch);
    const to = second
      ? from + first.length + rawMatch.slice(firstOffsetInMatch + first.length).search(/[A-Za-z_][\w$]*/) + second.length
      : from + first.length;

    if (second) {
      const hasSchema = schemas.has(firstLower);
      const tablesInSchema = schemaTables.get(firstLower);
      const hasTableInSchema = !!tablesInSchema?.has(secondLower);

      if (!hasSchema || !hasTableInSchema) {
        if (isSystemSchema(db, firstLower)) {
          refs.push({
            schema: firstLower,
            table: secondLower,
            columns: getSystemTableColumns(db, firstLower, secondLower),
            from,
            to,
          });
          continue;
        }

        diagnostics.push({ from, to, message: `Unknown table ${first}.${second}` });
        continue;
      }

      const cols = tableColumns.get(secondLower) ?? new Set<string>();
      refs.push({ schema: firstLower, table: secondLower, columns: cols, from, to });
      continue;
    }

    const cols = tableColumns.get(firstLower);
    if (!cols) {
      diagnostics.push({ from, to, message: `Unknown table ${first}` });
      continue;
    }

    refs.push({ schema: '', table: firstLower, columns: cols, from, to });
  }

  return refs;
}

function removeSelectAlias(expression: string): string {
  const withoutAs = expression.replace(/\s+AS\s+[A-Za-z_][\w$]*$/i, '');
  if (withoutAs !== expression) return withoutAs;

  const implicitAlias = /\s+[A-Za-z_][\w$]*$/.exec(expression);
  if (!implicitAlias) return expression;

  const beforeAlias = expression.slice(0, implicitAlias.index).trimEnd();
  return beforeAlias || expression;
}

function findColumnReferences(expression: string): Array<{
  table?: string;
  column: string;
  from: number;
  to: number;
}> {
  const refs: Array<{ table?: string; column: string; from: number; to: number }> = [];
  const tokenRe = /([A-Za-z_][\w$]*)(?:\s*\.\s*([A-Za-z_][\w$]*))?/g;

  let m: RegExpExecArray | null;
  while ((m = tokenRe.exec(expression)) !== null) {
    const first = m[1];
    const second = m[2];
    const afterMatch = expression.slice(tokenRe.lastIndex);
    const isFunctionName = !second && /^\s*\(/.test(afterMatch);

    if (isFunctionName || RESERVED.has(first.toUpperCase())) continue;

    if (second) {
      const secondOffset = m[0].lastIndexOf(second);
      refs.push({
        table: normalizeIdent(first),
        column: normalizeIdent(second),
        from: m.index + secondOffset,
        to: m.index + secondOffset + second.length,
      });
      continue;
    }

    refs.push({
      column: normalizeIdent(first),
      from: m.index,
      to: m.index + first.length,
    });
  }

  return refs;
}

export function findSqlSemanticDiagnostics(sqlText: string, db: DbSchema): SqlSemanticDiagnostic[] {
  const diagnostics: SqlSemanticDiagnostic[] = [];
  const refs = resolveTableReferences(sqlText, db, diagnostics);

  const selectMatch = /\bSELECT\b([\s\S]*?)\bFROM\b/i.exec(sqlText);
  if (!selectMatch) return diagnostics;

  const selectBody = selectMatch[1];
  if (!selectBody) return diagnostics;

  const selectStart = (selectMatch.index ?? 0) + selectMatch[0].indexOf(selectBody);
  const exprs = splitSelectExpressions(selectBody);

  const refByTable = new Map<string, TableRef>();
  for (const r of refs) refByTable.set(r.table, r);

  for (const expr of exprs) {
    const trimmed = expr.text.trim();
    if (!trimmed || trimmed === '*' || /\bDISTINCT\b\s*\*?/i.test(trimmed)) continue;

    const aliasRemoved = removeSelectAlias(trimmed);
    const columnRefs = findColumnReferences(aliasRemoved);

    for (const columnRef of columnRefs) {
      if (columnRef.table) {
        const ref = refByTable.get(columnRef.table);
        if (ref?.columns && !ref.columns.has(columnRef.column)) {
          const from = selectStart + expr.from + columnRef.from;
          diagnostics.push({
            from,
            to: selectStart + expr.from + columnRef.to,
            message: `Unknown column ${aliasRemoved.slice(columnRef.from, columnRef.to)}`,
          });
        }
        continue;
      }

      let foundInAny = false;
      for (const r of refs) {
        if (r.columns === null || r.columns.has(columnRef.column)) {
          foundInAny = true;
          break;
        }
      }

      if (!foundInAny && refs.length > 0) {
        const from = selectStart + expr.from + columnRef.from;
        diagnostics.push({
          from,
          to: selectStart + expr.from + columnRef.to,
          message: `Unknown column ${aliasRemoved.slice(columnRef.from, columnRef.to)}`,
        });
      }
    }
  }

  return diagnostics;
}

// ─── Smart completion source ──────────────────────────────────────────────────

/**
 * Creates a CodeMirror 6 CompletionSource that understands:
 *
 *   schema.table.col   – three-part qualified name
 *   alias.col          – alias resolved from the FROM / JOIN clauses
 *   table.col          – unqualified table name + column
 *   schema.table       – schema prefix → table list
 *   <word>             – schemas, tables, and columns (context-weighted)
 *
 * Intended to be registered on the SQL language data alongside (not instead
 * of) the built-in schemaCompletionSource so keyword completion still works:
 *
 *   sqlLang.language.data.of({ autocomplete: makeSmartCompletionSource(db) })
 */
export function makeSmartCompletionSource(db: DbSchema) {
  return function smartSqlComplete(
    ctx: CompletionContext,
  ): CompletionResult | null {
    const before = ctx.state.sliceDoc(0, ctx.pos);

    // ── Three-part: schema . table . partial ──────────────────────────────
    const threePart = /(\w+)\.(\w+)\.(\w*)$/.exec(before);
    if (threePart) {
      const [, schemaName, tableName, partial] = threePart;
      const from = ctx.pos - partial.length;

      if (db.schemas) {
        const schema = db.schemas.find(
          s => s.name.toLowerCase() === schemaName.toLowerCase(),
        );
        if (schema) {
          const table = schema.tables.find(
            t => t.name.toLowerCase() === tableName.toLowerCase(),
          );
          if (table) {
            return {
              from,
              options: table.columns
                .filter(c => c.name.toLowerCase().startsWith(partial.toLowerCase()))
                .map(colCompletion),
              validFor: /^\w*$/,
            };
          }
        }
      }
      // Fall through — let built-in handle it
    }

    // ── Two-part: identifier . partial ───────────────────────────────────
    const twoPart = /(\w+)\.(\w*)$/.exec(before);
    if (twoPart) {
      const [, prefix, partial] = twoPart;
      const from   = ctx.pos - partial.length;
      const lower  = prefix.toLowerCase();
      const fullSql = ctx.state.doc.toString();

      const aliases = extractAliases(fullSql, db);

      // 1. Alias or unqualified table name → columns
      const aliasCols = aliases.get(prefix) ?? aliases.get(lower);
      if (aliasCols && aliasCols.length > 0) {
        return {
          from,
          options: aliasCols
            .filter(c => c.name.toLowerCase().startsWith(partial.toLowerCase()))
            .map(colCompletion),
          validFor: /^\w*$/,
        };
      }

      // 2. Schema name → table list
      if (db.schemas) {
        const schema = db.schemas.find(
          s => s.name.toLowerCase() === lower,
        );
        if (schema) {
          return {
            from,
            options: schema.tables
              .filter(t => t.name.toLowerCase().startsWith(partial.toLowerCase()))
              .map(tableCompletion),
            validFor: /^\w*$/,
          };
        }
      }

      // 3. No match — return null and let the built-in source try
      return null;
    }

    // ── No dot: free-standing word completion ─────────────────────────────
    const word = ctx.matchBefore(/\w+/);
    if (!word && !ctx.explicit) return null;

    const partial = (word?.text ?? '').toLowerCase();
    const from    = word?.from ?? ctx.pos;

    // Determine if we're in a position where column names are useful.
    // Heuristic: look back (up to 200 chars) for SELECT, WHERE, SET, ON,
    // HAVING, RETURNING — if found before the next statement boundary, boost
    // column completions.
    const lookBack   = ctx.state.sliceDoc(Math.max(0, ctx.pos - 200), ctx.pos);
    const inColCtx   = /\b(SELECT|WHERE|SET|ON|HAVING|RETURNING|BY)\b/i.test(lookBack);
    const inFromCtx  = /\b(FROM|JOIN)\s*$/i.test(lookBack.trimEnd());

    const options: Completion[] = [];
    const seen = new Set<string>();

    function add(c: Completion) {
      if (!seen.has(c.label)) {
        seen.add(c.label);
        options.push(c);
      }
    }

    if (db.schemas) {
      for (const s of db.schemas) {
        if (s.name.toLowerCase().startsWith(partial)) {
          add(schemaCompletion(s.name));
        }
        for (const t of s.tables) {
          if (t.name.toLowerCase().startsWith(partial)) {
            const comp = tableCompletion(t);
            // Boost tables when we're right after FROM / JOIN
            add(inFromCtx ? { ...comp, boost: (comp.boost ?? 0) + 4 } : comp);
          }
          if (inColCtx) {
            for (const c of t.columns) {
              if (c.name.toLowerCase().startsWith(partial)) {
                add({ ...colCompletion(c), boost: (colCompletion(c).boost ?? 0) - 2 });
              }
            }
          }
        }
      }
    } else {
      // Flat (SQLite)
      const all = [...(db.tables ?? []), ...(db.views ?? [])];
      for (const t of all) {
        if (t.name.toLowerCase().startsWith(partial)) {
          const comp = tableCompletion(t);
          add(inFromCtx ? { ...comp, boost: (comp.boost ?? 0) + 4 } : comp);
        }
        if (inColCtx) {
          for (const c of t.columns) {
            if (c.name.toLowerCase().startsWith(partial)) {
              add({ ...colCompletion(c), boost: (colCompletion(c).boost ?? 0) - 2 });
            }
          }
        }
      }
    }

    if (options.length === 0) return null;

    return { from, options, validFor: /^\w*$/ };
  };
}
