import { writable, derived, get } from "svelte/store";
import type {
  connections,
  schema,
  history,
  main,
} from "../../tauri/gen/models";
import { GetSchema, LoadSchema, SaveSchema } from "../../tauri/gen/main/App";

// -----------------------------------------------------------------------
// Connection state
// -----------------------------------------------------------------------

export type ConnectionConfig = connections.ConnectionConfig;
export type SchemaTree = schema.SchemaTree;
export type QueryRecord = history.QueryRecord;
export type SavedQuery = history.SavedQuery;
export type ExecuteResult = main.ExecuteResult;

export interface ActiveConnection {
  config: ConnectionConfig;
  schema: SchemaTree | null;
  schemaLoading: boolean;
  schemaError: string | null;
}

export const activeConnections = writable<ActiveConnection[]>([]);
export const selectedConnId = writable<string>("");

export interface ServerGroup {
  id: string;
  title: string;
  connectionIds: string[];
}

const serverGroupsStorageKey = "multidb.serverGroups.v1";
const fontScaleStorageKey = "multidb.fontScalePercent.v1";
const connectionOrderStorageKey = "multidb.connectionOrder.v1";

function readStoredServerGroups(): ServerGroup[] {
  if (typeof localStorage === "undefined") return [];
  try {
    const raw = localStorage.getItem(serverGroupsStorageKey);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((group) => group && typeof group.id === "string")
      .map((group) => ({
        id: group.id,
        title: typeof group.title === "string" ? group.title : "Server Group",
        connectionIds: Array.isArray(group.connectionIds)
          ? group.connectionIds.filter((id: unknown) => typeof id === "string")
          : [],
      }));
  } catch (_) {
    return [];
  }
}

function readStoredFontScale(): number {
  if (typeof localStorage === "undefined") return 100;
  const stored = Number(localStorage.getItem(fontScaleStorageKey));
  return Number.isFinite(stored) && stored > 0 ? stored : 100;
}

function readStoredConnectionOrder(): string[] {
  if (typeof localStorage === "undefined") return [];
  try {
    const raw = localStorage.getItem(connectionOrderStorageKey);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((id): id is string => typeof id === "string");
  } catch (_) {
    return [];
  }
}

function applyStoredConnectionOrder(connections: ActiveConnection[]): ActiveConnection[] {
  const order = readStoredConnectionOrder();
  if (order.length === 0) return connections;

  const rank = new Map(order.map((id, index) => [id, index]));
  return [...connections].sort((a, b) => {
    const ai = rank.get(a.config.id);
    const bi = rank.get(b.config.id);
    if (ai === undefined && bi === undefined) return 0;
    if (ai === undefined) return 1;
    if (bi === undefined) return -1;
    return ai - bi;
  });
}

function clampFontScale(value: number): number {
  if (!Number.isFinite(value)) return 100;
  return Math.max(50, Math.min(250, Math.round(value)));
}

export const serverGroups = writable<ServerGroup[]>(readStoredServerGroups());
export const activeServerGroupId = writable<string>("");
export const fontScalePercent = writable<number>(
  clampFontScale(readStoredFontScale()),
);

serverGroups.subscribe((groups) => {
  if (typeof localStorage === "undefined") return;
  localStorage.setItem(serverGroupsStorageKey, JSON.stringify(groups));
});

fontScalePercent.subscribe((value) => {
  const scale = clampFontScale(value);
  if (typeof localStorage !== "undefined") {
    localStorage.setItem(fontScaleStorageKey, String(scale));
  }
  if (typeof document !== "undefined") {
    document.documentElement.style.setProperty(
      "--app-font-scale",
      String(scale / 100),
    );
  }
});

activeConnections.subscribe((connections) => {
  if (typeof localStorage === "undefined") return;
  localStorage.setItem(
    connectionOrderStorageKey,
    JSON.stringify(connections.map((conn) => conn.config.id)),
  );
});

export function setFontScalePercent(value: number) {
  fontScalePercent.set(clampFontScale(value));
}

export function setActiveConnectionsOrdered(connections: ActiveConnection[]) {
  activeConnections.set(applyStoredConnectionOrder(connections));
}

export function upsertActiveConnection(connection: ActiveConnection) {
  activeConnections.update((connections) => {
    const index = connections.findIndex(
      (existing) => existing.config.id === connection.config.id,
    );
    if (index === -1) {
      return [...connections, connection];
    }
    const next = [...connections];
    next[index] = connection;
    return next;
  });
}

export function addServerGroup(title: string) {
  const trimmed = title.trim();
  const id = crypto.randomUUID();
  serverGroups.update((groups) => [
    ...groups,
    { id, title: trimmed || "Server Group", connectionIds: [] },
  ]);
  activeServerGroupId.set(id);
  return id;
}

export function addConnectionToGroup(connId: string, groupId: string) {
  serverGroups.update((groups) =>
    groups.map((group) => ({
      ...group,
      connectionIds:
        group.id === groupId
          ? Array.from(new Set([...group.connectionIds, connId]))
          : group.connectionIds.filter((id) => id !== connId),
    })),
  );
}

export function removeConnectionFromGroups(connId: string) {
  serverGroups.update((groups) =>
    groups.map((group) => ({
      ...group,
      connectionIds: group.connectionIds.filter((id) => id !== connId),
    })),
  );
}

export function moveConnectionInList(
  connId: string,
  targetConnId: string | null,
  placement: "before" | "after" | "end",
  groupId?: string,
) {
  if (groupId) {
    serverGroups.update((groups) =>
      groups.map((group) => {
        const withoutMoved = group.connectionIds.filter((id) => id !== connId);
        if (group.id !== groupId) {
          return { ...group, connectionIds: withoutMoved };
        }

        const next = [...withoutMoved];
        const targetIndex = targetConnId
          ? next.findIndex((id) => id === targetConnId)
          : -1;
        const insertIndex =
          placement === "end" || targetIndex === -1
            ? next.length
            : placement === "after"
              ? targetIndex + 1
              : targetIndex;
        next.splice(insertIndex, 0, connId);
        return { ...group, connectionIds: next };
      }),
    );
    return;
  }

  removeConnectionFromGroups(connId);
  activeConnections.update((connections) => {
    const moved = connections.find((conn) => conn.config.id === connId);
    if (!moved) return connections;

    const next = connections.filter((conn) => conn.config.id !== connId);
    const targetIndex = targetConnId
      ? next.findIndex((conn) => conn.config.id === targetConnId)
      : -1;
    const insertIndex =
      placement === "end" || targetIndex === -1
        ? next.length
        : placement === "after"
          ? targetIndex + 1
          : targetIndex;
    next.splice(insertIndex, 0, moved);
    return next;
  });
}

export function moveServerGroup(
  groupId: string,
  targetGroupId: string | null,
  placement: "before" | "after",
) {
  serverGroups.update((groups) => {
    const moved = groups.find((g) => g.id === groupId);
    if (!moved) return groups;

    const withoutMoved = groups.filter((g) => g.id !== groupId);
    if (!targetGroupId) return [...withoutMoved, moved];

    const next = [...withoutMoved];
    const targetIndex = next.findIndex((g) => g.id === targetGroupId);
    if (targetIndex === -1) return [...withoutMoved, moved];

    const insertIndex =
      placement === "after" ? targetIndex + 1 : targetIndex;
    next.splice(insertIndex, 0, moved);
    return next;
  });
}

// -----------------------------------------------------------------------
// Tab / editor state
// -----------------------------------------------------------------------

export interface TabEditInfo {
  tableName: string;
  schemaName: string;
  primaryKeyCols: string[];
}

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
  editInfo: TabEditInfo | null;
  pendingEdits: Record<string, Record<string, any>>;
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
    editInfo: null,
    pendingEdits: {},
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
    closeTabsForConn(connId: string) {
      update((tabs) => {
        const remaining = tabs.filter((t) => t.connId !== connId);
        return remaining.length > 0 ? remaining : [makeTab()];
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
export const showImportDialog = writable(false);
export const importDialogConnId = writable<string>("");

export const outputTab = writable<"results" | "messages" | "history" | "saved">(
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

    // LoadSchema now returns a struct: { schemaJson, lastRefreshedAt, hash }
    const schemaJson: string = res.schemaJson ?? res.schema_json ?? (typeof res === 'string' ? res : '');
    if (!schemaJson) return;

    const schema = JSON.parse(schemaJson) as SchemaTree;
    const lastRefreshedAt: string = res.lastRefreshedAt ?? res.last_refreshed_at ?? new Date().toISOString();
    const hash: string = res.hash ?? '';

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

export async function refreshConnectionSchema(connId: string) {
  const conn = get(activeConnections).find((c) => c.config.id === connId);
  if (!conn || conn.schemaLoading) return;

  activeConnections.update((conns) =>
    conns.map((c) =>
      c.config.id === connId
        ? { ...c, schemaLoading: true, schemaError: null }
        : c,
    ),
  );

  try {
    const tree = await GetSchema(connId);
    activeConnections.update((conns) =>
      conns.map((c) =>
        c.config.id === connId
          ? { ...c, schema: tree, schemaLoading: false }
          : c,
      ),
    );
    const hash = JSON.stringify(tree);
    await saveCachedSchema(connId, tree, hash);
  } catch (e) {
    activeConnections.update((conns) =>
      conns.map((c) =>
        c.config.id === connId
          ? { ...c, schemaLoading: false, schemaError: String(e) }
          : c,
      ),
    );
  }
}

export async function refreshMissingConnectionSchemas() {
  const conns = get(activeConnections).filter(
    (conn) => !conn.schema && !conn.schemaLoading,
  );
  for (const conn of conns) {
    void refreshConnectionSchema(conn.config.id);
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
