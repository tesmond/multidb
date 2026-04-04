<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { tabs, activeConnections, selectedConnId, statusMessage, outputTab } from '../stores/appStore';
  import { ExecuteQuery, CancelQuery } from '../../wailsjs/go/main/App';
  import { get } from 'svelte/store';

  // CodeMirror 6
  import { EditorView, keymap, placeholder } from '@codemirror/view';
  import { EditorState, Compartment } from '@codemirror/state';
  import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
  import { sql, MySQL, PostgreSQL, SQLite } from '@codemirror/lang-sql';
  import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from '@codemirror/autocomplete';
  import { oneDark } from '@codemirror/theme-one-dark';
  import { lineNumbers, highlightActiveLineGutter, highlightActiveLine } from '@codemirror/view';
  import { bracketMatching, indentOnInput } from '@codemirror/language';
  import { highlightSelectionMatches, searchKeymap } from '@codemirror/search';

  export let tabId: string;

  $: tab = $tabs.find(t => t.id === tabId);

  let editorEl: HTMLDivElement;
  let view: EditorView | null = null;
  let sqlCompartment = new Compartment();

  // Build a schema map for CodeMirror SQL autocomplete from stored schema data
  function buildSchemaForCodemirror(connId: string): Record<string, string[]> {
    const conn = get(activeConnections).find(c => c.config.id === connId);
    if (!conn?.schema) return {};

    const result: Record<string, string[]> = {};

    function addTable(table: { name: string; columns?: { name: string }[] }) {
      result[table.name] = (table.columns ?? []).map(c => c.name);
    }

    if (conn.schema.schemas?.length) {
      for (const s of conn.schema.schemas) {
        for (const t of s.tables ?? []) addTable(t);
        for (const v of s.views ?? []) addTable(v);
      }
    } else {
      for (const t of conn.schema.tables ?? []) addTable(t);
      for (const v of conn.schema.views ?? []) addTable(v);
    }

    return result;
  }

  function getDialect(connId: string) {
    const conn = get(activeConnections).find(c => c.config.id === connId);
    switch (conn?.config.driver) {
      case 'mysql': return MySQL;
      case 'postgres': return PostgreSQL;
      case 'sqlite': return SQLite;
      default: return PostgreSQL;
    }
  }

  function makeSqlExtension(connId: string) {
    const schema = buildSchemaForCodemirror(connId);
    const dialect = getDialect(connId);
    return sql({ dialect, schema, upperCaseKeywords: true });
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

    const queryId = crypto.randomUUID();
    tabs.updateTab(tabId, { running: true, queryId, result: null, connId });
    statusMessage.set('Running query…');
    outputTab.set('results');

    try {
      const result = await ExecuteQuery(connId, queryId, sql, 1000000);
      tabs.updateTab(tabId, { running: false, result, queryId: '' });
      if (result.error) {
        statusMessage.set(`Error: ${result.error}`);
        outputTab.set('messages');
      } else {
        statusMessage.set(`${result.rows?.length ?? 0} rows · ${result.duration}ms`);
      }
    } catch (e: any) {
      tabs.updateTab(tabId, { running: false, queryId: '', result: { columns: [], rows: [], rowsAffected: 0, duration: 0, error: String(e) } });
      statusMessage.set(`Error: ${e}`);
      outputTab.set('messages');
    }
  }

  async function cancelQuery() {
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
          autocompletion(),
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
