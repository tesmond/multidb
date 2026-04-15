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
