import { writable, derived, get } from 'svelte/store';
import type { connections, schema, history, main } from '../../wailsjs/go/models';

// -----------------------------------------------------------------------
// Connection state
// -----------------------------------------------------------------------

export type ConnectionConfig = connections.ConnectionConfig;
export type SchemaTree = schema.SchemaTree;
export type QueryRecord = history.QueryRecord;
export type ExecuteResult = main.ExecuteResult;

export interface ActiveConnection {
  config: ConnectionConfig;
  schema: SchemaTree | null;
  schemaLoading: boolean;
  schemaError: string | null;
}

export const activeConnections = writable<ActiveConnection[]>([]);
export const selectedConnId = writable<string>('');

// -----------------------------------------------------------------------
// Tab / editor state
// -----------------------------------------------------------------------

export interface Tab {
  id: string;
  title: string;
  connId: string;
  sql: string;
  result: ExecuteResult | null;
  running: boolean;
  queryId: string;
}

function makeTab(connId = ''): Tab {
  const id = crypto.randomUUID();
  return { id, title: 'Query', connId, sql: '', result: null, running: false, queryId: '' };
}

function createTabStore() {
  const { subscribe, update, set } = writable<Tab[]>([makeTab()]);

  return {
    subscribe,
    add(connId: string) {
      update(tabs => [...tabs, makeTab(connId)]);
    },
    remove(id: string) {
      update(tabs => {
        const next = tabs.filter(t => t.id !== id);
        return next.length > 0 ? next : [makeTab()];
      });
    },
    updateTab(id: string, patch: Partial<Tab>) {
      update(tabs => tabs.map(t => (t.id === id ? { ...t, ...patch } : t)));
    },
    set,
  };
}

export const tabs = createTabStore();
export const activeTabId = writable<string>('');

// Ensure the active tab id is always valid
tabs.subscribe($tabs => {
  const $active = get(activeTabId);
  if (!$tabs.find(t => t.id === $active)) {
    activeTabId.set($tabs[0]?.id ?? '');
  }
});

export const activeTab = derived([tabs, activeTabId], ([$tabs, $id]) =>
  $tabs.find(t => t.id === $id) ?? null
);

// -----------------------------------------------------------------------
// UI state
// -----------------------------------------------------------------------

export const showConnectionDialog = writable(false);
export const editingConnection = writable<ConnectionConfig | null>(null);

export const outputTab = writable<'results' | 'messages' | 'history'>('results');

export const statusMessage = writable('Ready');
export const queryHistoryStore = writable<QueryRecord[]>([]);
