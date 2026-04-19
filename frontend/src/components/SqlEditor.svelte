<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { tabs, activeConnections, selectedConnId, statusMessage, outputTab, requestSchemaRefresh, extractFirstTableName } from '../stores/appStore';
  import { ExecuteQueryStreamed, CancelQuery } from '../../wailsjs/go/main/App';
  import { EventsOn, EventsOff } from '../../wailsjs/runtime/runtime';
  import { get } from 'svelte/store';

  // CodeMirror 6
  import { EditorView, keymap, placeholder } from '@codemirror/view';
  import { EditorState, Compartment } from '@codemirror/state';
  import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
  import { sql, MySQL, PostgreSQL, SQLite } from '@codemirror/lang-sql';
  import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from '@codemirror/autocomplete';
  import { buildSqlNamespace, makeSmartCompletionSource, type DbSchema } from '../lib/sqlComplete';
  import { oneDark } from '@codemirror/theme-one-dark';
  import { lineNumbers, highlightActiveLineGutter, highlightActiveLine } from '@codemirror/view';
  import { bracketMatching, indentOnInput } from '@codemirror/language';
  import { highlightSelectionMatches, searchKeymap } from '@codemirror/search';

  export let tabId: string;

  $: tab = $tabs.find(t => t.id === tabId);

  // Cleanup function for the active streaming query's event listeners.
  // Replaced on each new query; called on cancel, destroy, and query start.
  let cancelListeners: (() => void) | null = null;

  let editorEl: HTMLDivElement;
  let view: EditorView | null = null;
  let sqlCompartment = new Compartment();

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
          rowsAffected: 0,
          duration: pendingDuration,
          error: pendingError,
        } as any,
      });
      if (pendingError) {
        statusMessage.set(`Error: ${pendingError}`);
        outputTab.set('messages');
      } else {
        statusMessage.set(`${streamRows.length} rows · ${pendingDuration}ms`);
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

    const offDone = EventsOn('query:done', (done: { queryId: string; totalRows: number; duration: number; error?: string }) => {
      if (done.queryId !== queryId) return;
      pendingTotalRows = done.totalRows;
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
    <select
      class="conn-select"
      bind:value={tab.connId}
      on:change={() => tabs.updateTab(tabId, { connId: tab?.connId ?? '' })}
    >
      <option value="">— select connection —</option>
      {#each $activeConnections as conn}
        <option value={conn.config.id}>{conn.config.name}</option>
      {/each}
    </select>

    {#if tab.running}
      <button class="btn-stop" on:click={cancelQuery} title="Cancel query (Ctrl+.)">⏹ Stop</button>
    {:else}
      <button class="btn-run" on:click={runQuery} title="Run query (Ctrl+Enter)">▶ Run</button>
    {/if}
  </div>

  <div class="cm-host" bind:this={editorEl} aria-label="SQL editor"></div>
</div>
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
  .conn-select {
    background: var(--bg-input); border: 1px solid var(--border);
    color: var(--text); padding: 5px 8px; border-radius: 4px;
    font-size: 12px; min-width: 160px;
  }
  .conn-select:focus { outline: none; border-color: var(--accent); }

  .btn-run, .btn-stop {
    padding: 5px 14px; border-radius: 4px; font-size: 12px;
    cursor: pointer; border: 1px solid transparent; font-weight: 500;
  }
  .btn-run { background: var(--accent); color: #fff; border-color: var(--accent); }
  .btn-run:hover { background: var(--accent-hover); }
  .btn-stop { background: var(--error); color: #fff; border-color: var(--error); }
  .btn-stop:hover { opacity: 0.85; }

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
</style>
