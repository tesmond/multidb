import { writable, derived, get } from "svelte/store";
import type {
  connections,
  schema,
  history,
  main,
} from "../../wailsjs/go/models";
import { LoadSchema, SaveSchema } from "../../wailsjs/go/main/App";

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
export const selectedConnId = writable<string>("");

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
  sortCol: number;
  sortDirection: "asc" | "desc";
  manuallyRenamed: boolean;
}

function makeTab(connId = ""): Tab {
  const id = crypto.randomUUID();
  return {
    id,
    title: "Query",
    connId,
    sql: "",
    result: null,
    running: false,
    queryId: "",
    sortCol: -1,
    sortDirection: "asc",
    manuallyRenamed: false,
  };
}

function createTabStore() {
  const { subscribe, update, set } = writable<Tab[]>([makeTab()]);

  return {
    subscribe,
    add(connId: string) {
      update((tabs) => [...tabs, makeTab(connId)]);
    },
    remove(id: string) {
      update((tabs) => {
        const next = tabs.filter((t) => t.id !== id);
        return next.length > 0 ? next : [makeTab()];
      });
    },
    updateTab(id: string, patch: Partial<Tab>) {
      update((tabs) => tabs.map((t) => (t.id === id ? { ...t, ...patch } : t)));
    },
    renameTab(id: string, newTitle: string) {
      update((tabs) =>
        tabs.map((t) =>
          t.id === id ? { ...t, title: newTitle, manuallyRenamed: true } : t,
        ),
      );
    },
    duplicateTab(id: string) {
      update((tabs) => {
        const index = tabs.findIndex((t) => t.id === id);
        if (index === -1) return tabs;
        const original = tabs[index];
        const duplicate: Tab = {
          ...original,
          id: crypto.randomUUID(),
          title: original.title + " (Copy)",
          manuallyRenamed: false,
        };
        const newTabs = [...tabs];
        newTabs.splice(index + 1, 0, duplicate);
        return newTabs;
      });
    },
    closeOtherTabs(id: string) {
      update((tabs) => tabs.filter((t) => t.id === id));
    },
    closeTabsToRight(id: string) {
      update((tabs) => {
        const index = tabs.findIndex((t) => t.id === id);
        if (index === -1) return tabs;
        return tabs.slice(0, index + 1);
      });
    },
    closeTabsToLeft(id: string) {
      update((tabs) => {
        const index = tabs.findIndex((t) => t.id === id);
        if (index === -1) return tabs;
        return tabs.slice(index);
      });
    },
    reorderTabs(fromIndex: number, toIndex: number) {
      update((tabs) => {
        const newTabs = [...tabs];
        const [moved] = newTabs.splice(fromIndex, 1);
        newTabs.splice(toIndex, 0, moved);
        return newTabs;
      });
    },
    set,
  };
}

export const tabs = createTabStore();
export const activeTabId = writable<string>("");

// Ensure the active tab id is always valid
tabs.subscribe(($tabs) => {
  const $active = get(activeTabId);
  if (!$tabs.find((t) => t.id === $active)) {
    activeTabId.set($tabs[0]?.id ?? "");
  }
});

export const activeTab = derived(
  [tabs, activeTabId],
  ([$tabs, $id]) => $tabs.find((t) => t.id === $id) ?? null,
);

// -----------------------------------------------------------------------
// UI state
// -----------------------------------------------------------------------

export const showConnectionDialog = writable(false);
export const editingConnection = writable<ConnectionConfig | null>(null);

export const outputTab = writable<"results" | "messages" | "history">(
  "results",
);

export const statusMessage = writable("Ready");
export const queryHistoryStore = writable<QueryRecord[]>([]);

// -----------------------------------------------------------------------
// Schema refresh signal
// -----------------------------------------------------------------------
// Components (SqlEditor) set this to trigger Navigator to re-fetch the schema
// for a given connection after a DDL statement is executed.

export const schemaRefreshSignal = writable<{
  connId: string;
  ts: number;
} | null>(null);

export function requestSchemaRefresh(connId: string) {
  schemaRefreshSignal.set({ connId, ts: Date.now() });
}

// -----------------------------------------------------------------------
// Tab utilities
// -----------------------------------------------------------------------

// Extract the first table name from a SQL query for dynamic tab naming.
// Looks for the first table in FROM or JOIN clauses, ignoring schema prefixes.
export function extractFirstTableName(sql: string): string | null {
  if (!sql) return null;
  const normalized = sql.replace(/\s+/g, " ").trim();
  // Attempt to find FROM or JOIN then an optional schema and table name
  const re =
    /\b(?:FROM|JOIN)\s+(?:["'`[]?[\w-]+["'`\]]?\s*\.\s*)?["'`[]?([\w-]+)["'`\]]?/i;
  const m = re.exec(normalized);
  return m ? m[1] : null;
}

// -----------------------------------------------------------------------
// Schema cache persistence
// -----------------------------------------------------------------------

// Simple in-memory cache (mirrors persisted cache on disk via backend)
// keyed by connection ID.
export const schemaCache = writable<
  Record<
    string,
    { schema: SchemaTree | null; lastRefreshedAt: string; hash: string }
  >
>({});

/**
 * loadCachedSchema
 * - Calls backend LoadSchema(connId)
 * - Accepts either a raw JSON string or an object { schemaJson, lastRefreshedAt, hash }
 * - Updates both `schemaCache` and `activeConnections` so the UI can render immediately
 */
export async function loadCachedSchema(connId: string) {
  try {
    const res: any = await LoadSchema(connId);
    if (!res) return;

    // Normalize possible response shapes
    const schemaJson: string | undefined =
      typeof res === "string" ? res : (res?.schemaJson ?? res?.schema_json);
    if (!schemaJson) return;

    const schema = JSON.parse(schemaJson) as SchemaTree;
    const lastRefreshedAt: string =
      res?.lastRefreshedAt ??
      res?.last_refreshed_at ??
      new Date().toISOString();
    const hash: string = res?.hash ?? res?.h ?? "";

    // Update stores
    schemaCache.update((cache) => ({
      ...cache,
      [connId]: { schema, lastRefreshedAt, hash },
    }));
    activeConnections.update((conns) =>
      conns.map((c) => (c.config.id === connId ? { ...c, schema } : c)),
    );
  } catch (e) {
    // Missing cache or parse failure — ignore
  }
}

/**
 * hydrateCachedSchemas
 * - Iterates activeConnections and loads cached schema for each
 * - Sequential to avoid backend overload on startup; can be parallelised later
 */
export async function hydrateCachedSchemas() {
  const conns = get(activeConnections);
  for (const conn of conns) {
    // loadCachedSchema already swallows errors
    // eslint-disable-next-line no-await-in-loop
    await loadCachedSchema(conn.config.id);
  }
}

/**
 * saveCachedSchema
 * - Persists schema via backend SaveSchema and updates local cache
 */
export async function saveCachedSchema(
  connId: string,
  schema: SchemaTree,
  hash: string,
) {
  try {
    const schemaJson = JSON.stringify(schema);
    await SaveSchema(connId, schemaJson, hash);
    schemaCache.update((cache) => ({
      ...cache,
      [connId]: { schema, lastRefreshedAt: new Date().toISOString(), hash },
    }));
  } catch (e) {
    // ignore persistence failures
  }
}

/**
 * deleteCachedSchema
 * - Remove entry from local cache (backend deletion not implemented here)
 */
export async function deleteCachedSchema(connId: string) {
  schemaCache.update((cache) => {
    const next = { ...cache };
    delete next[connId];
    return next;
  });
}
