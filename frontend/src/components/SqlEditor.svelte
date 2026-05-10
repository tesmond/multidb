<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { tabs, activeConnections, selectedConnId, statusMessage, outputTab, requestSchemaRefresh, extractFirstTableName } from '../stores/appStore';
  import { ExecuteQueryStreamed, CancelQuery } from '../../wailsjs/go/main/App';
  import { EventsOn, EventsOff } from '../../wailsjs/runtime/runtime';
  import { get } from 'svelte/store';
  import SaveQueryDialog from './SaveQueryDialog.svelte';

  // CodeMirror 6
  import { EditorView, keymap, placeholder } from '@codemirror/view';
  import { EditorState, Compartment } from '@codemirror/state';
  import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
  import { sql, MySQL, PostgreSQL, SQLite } from '@codemirror/lang-sql';
  import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from '@codemirror/autocomplete';
  import { buildSqlNamespace, makeSmartCompletionSource, findSqlSemanticDiagnostics, findSqlCommonSyntaxDiagnostics, type DbSchema } from '../lib/sqlComplete';
  import { oneDark } from '@codemirror/theme-one-dark';
  import { lineNumbers, highlightActiveLineGutter, highlightActiveLine } from '@codemirror/view';
  import { bracketMatching, indentOnInput, syntaxTree } from '@codemirror/language';
  import { highlightSelectionMatches, searchKeymap } from '@codemirror/search';
  import { linter, lintGutter, type Diagnostic } from '@codemirror/lint';
  import ConnectionSelect from './ConnectionSelect.svelte';

  export let tabId: string;

  $: tab = $tabs.find(t => t.id === tabId);
  $: connectionOptions = [
    { value: '', label: '— select connection —' },
    ...$activeConnections.map((conn) => ({ value: conn.config.id, label: conn.config.name })),
  ];

  // Cleanup function for the active streaming query's event listeners.
  // Replaced on each new query; called on cancel, destroy, and query start.
  let cancelListeners: (() => void) | null = null;

  let editorEl: HTMLDivElement;
  let view: EditorView | null = null;
  let sqlCompartment = new Compartment();

  // Combined linter: parser errors + schema-aware semantic identifier checks.
  const sqlLinter = linter((editorView) => {
    const docText = editorView.state.doc.toString();
    const diagnostics: Diagnostic[] = [];

    const syntax = findSqlCommonSyntaxDiagnostics(docText);
    for (const d of syntax) {
      diagnostics.push({
        from: d.from,
        to: d.to,
        severity: 'error',
        message: d.message,
      });
    }

    const activeTab = get(tabs).find(t => t.id === tabId);
    const connId = activeTab?.connId || get(selectedConnId) || '';
    const dbSchema = getConnectionDbSchema(connId);

    if (dbSchema) {
      const semantic = findSqlSemanticDiagnostics(docText, dbSchema);
      for (const d of semantic) {
        diagnostics.push({
          from: d.from,
          to: d.to,
          severity: 'error',
          message: d.message,
        });
      }
    }

    const tree = syntaxTree(editorView.state);
    tree.cursor().iterate(node => {
      if (node.type.isError) {
        // Expand zero-length error ranges by one char so the underline is visible.
        const from = node.from;
        const to   = node.to > node.from ? node.to : Math.min(node.from + 1, editorView.state.doc.length);
        diagnostics.push({
          from,
          to,
          severity: 'error',
          message: 'SQL syntax error',
        });
      }
    });
    return diagnostics;
  }, { delay: 100 });
  let saveQueryDialog: SaveQueryDialog;

  // Convert the active-connection schema into the DbSchema shape used by
  // the smart completion engine.
  function getConnectionDbSchema(connId: string): DbSchema | null {
    const conn = get(activeConnections).find(c => c.config.id === connId);
    if (!conn?.schema) return null;

    const driver = conn.config.driver ?? 'postgres';

    if (conn.schema.schemas?.length) {
      return {
        driver,
        schemas: conn.schema.schemas.map(s => ({
          name: s.name,
          tables: [
            ...(s.tables ?? []).map(t => ({
              name: t.name,
              columns: (t.columns ?? []).map(c => ({ name: c.name, type: c.type, key: c.key })),
              isView: false,
            })),
            ...(s.views ?? []).map(t => ({
              name: t.name,
              columns: (t.columns ?? []).map(c => ({ name: c.name, type: c.type, key: c.key })),
              isView: true,
            })),
          ],
        })),
      };
    }

    return {
      driver,
      tables: (conn.schema.tables ?? []).map(t => ({
        name: t.name,
        columns: (t.columns ?? []).map(c => ({ name: c.name, type: c.type, key: c.key })),
        isView: false,
      })),
      views: (conn.schema.views ?? []).map(t => ({
        name: t.name,
        columns: (t.columns ?? []).map(c => ({ name: c.name, type: c.type, key: c.key })),
        isView: true,
      })),
    };
  }

  function getDialect(connId: string) {
    const conn = get(activeConnections).find(c => c.config.id === connId);
    switch (conn?.config.driver) {
      case 'mysql':    return MySQL;
      case 'postgres': return PostgreSQL;
      case 'sqlite':   return SQLite;
      default:         return PostgreSQL;
    }
  }

  /**
   * Returns true for any SQL statement that modifies the schema
   * (DDL) so we know to refresh the navigator tree afterwards.
   */
  function isDDL(sqlText: string): boolean {
    return /^\s*(?:CREATE|DROP|ALTER|RENAME|TRUNCATE|COMMENT\s+ON)\b/im.test(sqlText);
  }

  function makeSqlExtension(connId: string) {
    const dialect  = getDialect(connId);
    const dbSchema = getConnectionDbSchema(connId);

    // Build hierarchical namespace for the built-in schema completion
    // (handles schema.table and table.column dot-completions natively).
    const namespace = dbSchema ? buildSqlNamespace(dbSchema) : {};

    const sqlLang = sql({ dialect, schema: namespace, upperCaseKeywords: true });

    if (dbSchema) {
      // Register our smart source on the language data so it runs
      // alongside (not instead of) the built-in keyword + schema completion.
      const smartSource = makeSmartCompletionSource(dbSchema);
      return [
        sqlLang.language.data.of({ autocomplete: smartSource }),
        sqlLang,
      ];
    }

    return sqlLang;
  }

  function getSelectedOrAllSQL(): string {
    if (!view) return tab?.sql ?? '';
    const sel = view.state.sliceDoc(
      view.state.selection.main.from,
      view.state.selection.main.to,
    ).trim();
    return sel || view.state.doc.toString();
  }

  async function runQuery() {
    if (!tab || tab.running) return;
    const sql = getSelectedOrAllSQL().trim();
    if (!sql) return;

    const connId = tab.connId || get(selectedConnId);
    if (!connId) {
      statusMessage.set('No connection selected. Please connect to a database first.');
      return;
    }

    // Clean up any lingering listeners from the previous query.
    cancelListeners?.();
    cancelListeners = null;

    const queryId = crypto.randomUUID();
    tabs.updateTab(tabId, { running: true, queryId, result: null, connId });
    statusMessage.set('Running query…');
    outputTab.set('results');

    // Local mutable accumulator – mutated in place; Svelte reactivity is
    // triggered by replacing the result wrapper object each chunk.
    let streamCols: string[] = [];
    let streamColTypes: string[] = [];
    let streamRows: any[][] = [];

    // Rendezvous state: finalize only once BOTH the done signal has arrived
    // AND all expected rows have been received. This handles the case where
    // Wails sends query:done and the last query:chunk as separate WebSocket
    // frames (separate macrotasks) that can arrive in either order.
    let pendingTotalRows = -1;  // -1 = done not yet received
    let pendingDuration = 0;
    let pendingRowsAffected = 0;
    let pendingError = '';

    function tryFinalize() {
      if (pendingTotalRows < 0) return;                    // done not received yet
      if (streamRows.length < pendingTotalRows) return;    // chunks still in-flight
      offMeta(); offChunk(); offDone();
      cancelListeners = null;
      tabs.updateTab(tabId, {
        running: false,
        queryId: '',
        result: {
          columns: streamCols,
          columnTypes: streamColTypes,
          rows: streamRows,
          _rowCount: streamRows.length,
          rowsAffected: pendingRowsAffected,
          duration: pendingDuration,
          error: pendingError,
        } as any,
      });
      if (pendingError) {
        statusMessage.set(`Error: ${pendingError}`);
        outputTab.set('messages');
      } else {
        if (pendingRowsAffected > 0 && streamRows.length === 0) {
          statusMessage.set(`${pendingRowsAffected} row(s) affected · ${pendingDuration}ms`);
        } else {
          statusMessage.set(`${streamRows.length} rows · ${pendingDuration}ms`);
        }
        // Automatically refresh the schema tree when a DDL statement
        // succeeds (CREATE TABLE, DROP TABLE, ALTER TABLE, etc.)
        if (isDDL(sql)) {
          requestSchemaRefresh(connId);
        }
        // Dynamic tab naming: if not manually renamed, set title to first table name
        const currentTab = get(tabs).find(t => t.id === tabId);
        if (currentTab && !currentTab.manuallyRenamed) {
          const tableName = extractFirstTableName(sql);
          if (tableName) {
            tabs.updateTab(tabId, { title: tableName });
          }
        }
      }
    }

    const offMeta = EventsOn('query:meta', (meta: { queryId: string; columns: string[]; columnTypes: string[] }) => {
      if (meta.queryId !== queryId) return;
      streamCols = meta.columns;
      streamColTypes = meta.columnTypes ?? [];
      streamRows = [];
      tabs.updateTab(tabId, {
        result: { columns: streamCols, columnTypes: streamColTypes, rows: streamRows, _rowCount: 0, rowsAffected: 0, duration: 0, error: '' } as any,
      });
    });

    const offChunk = EventsOn('query:chunk', (chunk: { queryId: string; rows: any[][] }) => {
      if (chunk.queryId !== queryId) return;
      const incoming = chunk.rows;
      for (let i = 0; i < incoming.length; i++) streamRows.push(incoming[i]);
      const rowCount = streamRows.length;
      tabs.updateTab(tabId, {
        result: { columns: streamCols, columnTypes: streamColTypes, rows: streamRows, _rowCount: rowCount, rowsAffected: 0, duration: 0, error: '' } as any,
      });
      statusMessage.set(`Loading… ${rowCount} rows`);
      tryFinalize(); // handle case: done arrived before this last chunk
    });

    const offDone = EventsOn('query:done', (done: { queryId: string; totalRows: number; rowsAffected?: number; duration: number; error?: string }) => {
      if (done.queryId !== queryId) return;
      pendingTotalRows = done.totalRows;
      pendingRowsAffected = done.rowsAffected ?? 0;
      pendingDuration = done.duration;
      pendingError = done.error ?? '';
      tryFinalize(); // handle case: all chunks already arrived
    });

    cancelListeners = () => { offMeta(); offChunk(); offDone(); };

    // Fire and forget – all coordination flows through the events above.
    ExecuteQueryStreamed(connId, queryId, sql, 1_000_000).catch((e: any) => {
      offMeta(); offChunk(); offDone();
      cancelListeners = null;
      tabs.updateTab(tabId, {
        running: false,
        queryId: '',
        result: { columns: [], columnTypes: [], rows: [], rowsAffected: 0, duration: 0, error: String(e) },
      });
      statusMessage.set(`Error: ${e}`);
      outputTab.set('messages');
    });
  }

  async function cancelQuery() {
    cancelListeners?.();
    cancelListeners = null;
    if (!tab?.queryId) return;
    await CancelQuery(tab.queryId);
    tabs.updateTab(tabId, { running: false, queryId: '' });
    statusMessage.set('Query cancelled');
  }

  async function saveQuery() {
    if (!tab) return;
    const sql = tab.sql.trim();
    if (!sql) {
      statusMessage.set('Cannot save empty query');
      return;
    }

    const connId = tab.connId || get(selectedConnId);
    if (!connId) {
      statusMessage.set('No connection selected. Please select a connection first.');
      return;
    }

    const title = await saveQueryDialog.open();
    if (!title) {
      // User cancelled
      return;
    }

    try {
      const app = await import('../../wailsjs/go/main/App') as any;
      await app.SaveQuery(connId, title, sql);
      statusMessage.set(`Query saved as "${title}"`);
      // Dispatch an event to refresh the saved queries list
      window.dispatchEvent(new Event('saved-query-added'));
    } catch (e: any) {
      statusMessage.set(`Error saving query: ${e}`);
    }
  }

  onMount(() => {
    const initialConnId = get(tabs).find(t => t.id === tabId)?.connId ?? '';

    view = new EditorView({
      parent: editorEl,
      state: EditorState.create({
        doc: tab?.sql ?? '',
        extensions: [
          oneDark,
          lineNumbers(),
          highlightActiveLineGutter(),
          highlightActiveLine(),
          bracketMatching(),
          closeBrackets(),
          indentOnInput(),
          highlightSelectionMatches(),
          history(),
          autocompletion({
            activateOnTyping: true,
            maxRenderedOptions: 50,
            defaultKeymap: true,
          }),
          lintGutter(),
          sqlLinter,
          sqlCompartment.of(makeSqlExtension(initialConnId)),
          keymap.of([
            { key: 'Ctrl-Enter', mac: 'Cmd-Enter', run: () => { runQuery(); return true; } },
            ...closeBracketsKeymap,
            ...defaultKeymap,
            ...historyKeymap,
            ...completionKeymap,
            ...searchKeymap,
            indentWithTab,
          ]),
          placeholder('Type SQL here… (Ctrl+Enter to run)'),
          EditorView.updateListener.of(update => {
            if (update.docChanged) {
              tabs.updateTab(tabId, { sql: update.state.doc.toString() });
            }
          }),
          EditorView.theme({
            '&': { height: '100%' },
            '.cm-scroller': { fontFamily: "'JetBrains Mono','Fira Code','Cascadia Code',monospace", fontSize: '13px', lineHeight: '1.6' },
            '.cm-content': { padding: '12px 0' },
          }),
        ],
      }),
    });

    // Keep CM in sync when the tab SQL is changed externally (e.g. from Navigator)
    const unsubscribe = tabs.subscribe($tabs => {
      const t = $tabs.find(t => t.id === tabId);
      if (!view || !t) return;
      const current = view.state.doc.toString();
      if (t.sql !== current) {
        view.dispatch({ changes: { from: 0, to: current.length, insert: t.sql } });
      }
    });

    // Refresh SQL dialect + schema when the connection changes
    const unsubConn = activeConnections.subscribe(() => {
      if (!view) return;
      const t = get(tabs).find(t => t.id === tabId);
      if (!t) return;
      view.dispatch({ effects: sqlCompartment.reconfigure(makeSqlExtension(t.connId)) });
    });

    return () => {
      unsubscribe();
      unsubConn();
    };
  });

  onDestroy(() => {
    cancelListeners?.();
    cancelListeners = null;
    view?.destroy();
    view = null;
  });

  // When connId changes on the tab, reconfigure the SQL dialect
  $: if (view && tab?.connId !== undefined) {
    view.dispatch({ effects: sqlCompartment.reconfigure(makeSqlExtension(tab.connId)) });
  }
</script>

{#if tab}
<div class="editor-wrap">
  <div class="editor-toolbar">
    <ConnectionSelect
      bind:value={tab.connId}
      options={connectionOptions}
      onchange={(connId) => tabs.updateTab(tabId, { connId })}
    />

    {#if tab.running}
      <button class="btn-stop" on:click={cancelQuery} title="Cancel query (Ctrl+.)">⏹ Stop</button>
    {:else}
      <button class="btn-run" on:click={runQuery} title="Run query (Ctrl+Enter)">▶ Run</button>
      <button
        class="btn-save"
        on:click={saveQuery}
        disabled={!tab.sql.trim()}
        title="Save query"
      >
        💾 Save
      </button>
    {/if}
  </div>

  <div class="cm-host" bind:this={editorEl} aria-label="SQL editor"></div>
</div>

<SaveQueryDialog bind:this={saveQueryDialog} />
{/if}

<style>
  .editor-wrap {
    display: flex; flex-direction: column;
    height: 100%;
  }
  .editor-toolbar {
    display: flex; align-items: center; gap: 8px;
    padding: 6px 10px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .btn-run, .btn-stop {
    padding: 5px 14px; border-radius: 4px; font-size: 12px;
    cursor: pointer; border: 1px solid transparent; font-weight: 500;
  }
  .btn-run { background: var(--accent); color: #fff; border-color: var(--accent); }
  .btn-run:hover { background: var(--accent-hover); }
  .btn-stop { background: var(--error); color: #fff; border-color: var(--error); }
  .btn-stop:hover { opacity: 0.85; }
  .btn-save { background: var(--accent-secondary, #336fc8); color: #fff; border-color: var(--accent-secondary, #48a868); padding: 5px 14px; border-radius: 4px; font-size: 12px; cursor: pointer; border: 1px solid transparent; font-weight: 500; }
  .btn-save:hover:not(:disabled) { opacity: 0.9; }
  .btn-save:disabled { opacity: 0.5; cursor: not-allowed; }

  .cm-host {
    flex: 1;
    overflow: hidden;
    min-height: 0;
  }

  .cm-host :global(.cm-editor) {
    height: 100%;
  }

  .cm-host :global(.cm-editor.cm-focused) {
    outline: none;
  }

  /* Don't let the syntax highlighter colour error/invalid tokens red —
     the squiggly underline from the linter is enough. */
  .cm-host :global(.cm-invalid) {
    color: inherit !important;
  }
</style>
