// Mock generated desktop bindings before any store imports
vi.mock('../../desktop/gen/models', () => ({
  connections: { ConnectionConfig: class {} },
  schema: { SchemaTree: class {} },
  history: { QueryRecord: class {} },
  main: { ExecuteResult: class {} },
}));

vi.mock('../../desktop/gen/main/App', () => ({
  GetSchema: vi.fn(),
  LoadSchema: vi.fn(),
  SaveSchema: vi.fn(),
}));

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import type { Tab } from './appStore';

// Dynamically import to allow mocks to apply first
let tabs: any;
let activeTabId: any;
let activeTab: any;
let activeConnections: any;
let statusMessage: any;
let outputTab: any;
let openRelationshipDiagramTab: any;
let showRelationshipDiagramForConnection: any;
let backendApp: any;

beforeEach(async () => {
  vi.resetModules();
  const mod = await import('./appStore');
  tabs = mod.tabs;
  activeTabId = mod.activeTabId;
  activeTab = mod.activeTab;
  activeConnections = mod.activeConnections;
  statusMessage = mod.statusMessage;
  outputTab = mod.outputTab;
  openRelationshipDiagramTab = mod.openRelationshipDiagramTab;
  showRelationshipDiagramForConnection = mod.showRelationshipDiagramForConnection;
  backendApp = await import('../../desktop/gen/main/App');
  vi.mocked(backendApp.LoadSchema).mockResolvedValue(null);
  vi.mocked(backendApp.GetSchema).mockResolvedValue({
    sizeBytes: undefined,
    tables: [],
    views: [],
    indexes: [],
    relationships: [],
    schemas: [],
  });
});

describe('tabs store', () => {
  it('starts with one empty tab', () => {
    const $tabs = get(tabs) as Tab[];
    expect($tabs).toHaveLength(1);
    expect($tabs[0].kind).toBe('sql');
    if ($tabs[0].kind !== 'sql') throw new Error('expected sql tab');
    expect($tabs[0].sql).toBe('');
    expect($tabs[0].title).toBe('Query');
  });

  it('add() creates a new tab', () => {
    tabs.add('conn-1');
    const $tabs = get(tabs) as Tab[];
    expect($tabs).toHaveLength(2);
    expect($tabs[1].connId).toBe('conn-1');
  });

  it('remove() removes the given tab', () => {
    tabs.add('conn-a');
    const id = (get(tabs) as Tab[])[0].id;
    tabs.remove(id);
    expect(get(tabs)).not.toContain(expect.objectContaining({ id }));
  });

  it('remove() keeps at least one tab', () => {
    const only = (get(tabs) as Tab[])[0].id;
    tabs.remove(only);
    expect(get(tabs)).toHaveLength(1);
  });

  it('updateTab() patches a single tab', () => {
    const id = (get(tabs) as Tab[])[0].id;
    tabs.updateTab(id, { sql: 'SELECT 1', running: true });
    const tab = (get(tabs) as any[]).find((t: any) => t.id === id);
    expect(tab?.sql).toBe('SELECT 1');
    expect(tab?.running).toBe(true);
  });

  it('openRelationshipDiagramTab() creates one diagram tab per connection and focuses it', () => {
    const firstTabId = (get(tabs) as Tab[])[0].id;

    const createdId = openRelationshipDiagramTab('conn-1', 'Sales DB');
    let $tabs = get(tabs) as any[];
    expect($tabs).toHaveLength(2);
    expect($tabs[1].id).toBe(createdId);
    expect($tabs[1].kind).toBe('relationshipDiagram');
    expect($tabs[1].connId).toBe('conn-1');
    expect($tabs[1].title).toBe('Sales DB Relationships');
    expect(get(activeTabId)).toBe(createdId);

    const focusedId = openRelationshipDiagramTab('conn-1', 'Ignored Title');
    $tabs = get(tabs) as any[];
    expect($tabs).toHaveLength(2);
    expect(focusedId).toBe(createdId);
    expect(get(activeTabId)).toBe(createdId);
    expect($tabs[0].id).toBe(firstTabId);
  });

  it('showRelationshipDiagramForConnection() loads schema before opening the diagram tab', async () => {
    activeConnections.set([
      {
        config: {
          id: 'conn-1',
          name: 'Sales DB',
          driver: 'sqlite',
          tabColor: '',
          tabTextBlack: false,
          host: '',
          port: 0,
          username: '',
          password: '',
          database: '',
          dsn: '',
          useKubePortForward: false,
          kubeContext: '',
          kubeNamespace: '',
          kubeResource: '',
          kubeLocalPort: 0,
          kubeRemotePort: 0,
        },
        schema: null,
        schemaLoading: false,
        schemaError: null,
      },
    ]);

    const openedId = await showRelationshipDiagramForConnection('conn-1');

    expect(backendApp.LoadSchema).toHaveBeenCalledWith('conn-1');
    expect(backendApp.GetSchema).toHaveBeenCalledWith('conn-1');
    expect(openedId).toEqual(expect.any(String));
    expect(get(tabs)).toContainEqual(
      expect.objectContaining({
        id: openedId,
        kind: 'relationshipDiagram',
        connId: 'conn-1',
        title: 'Sales DB Relationships',
      }),
    );
    expect(get(activeTabId)).toBe(openedId);
  });

  it('showRelationshipDiagramForConnection() reuses the existing relationship tab when schema is already loaded', async () => {
    activeConnections.set([
      {
        config: {
          id: 'conn-1',
          name: 'Sales DB',
          driver: 'sqlite',
          tabColor: '',
          tabTextBlack: false,
          host: '',
          port: 0,
          username: '',
          password: '',
          database: '',
          dsn: '',
          useKubePortForward: false,
          kubeContext: '',
          kubeNamespace: '',
          kubeResource: '',
          kubeLocalPort: 0,
          kubeRemotePort: 0,
        },
        schema: {
          sizeBytes: 0,
          tables: [],
          views: [],
          indexes: [],
          relationships: [],
          schemas: [],
        },
        schemaLoading: false,
        schemaError: null,
      },
    ]);

    vi.mocked(backendApp.LoadSchema).mockClear();
    vi.mocked(backendApp.GetSchema).mockClear();

    const firstId = await showRelationshipDiagramForConnection('conn-1');
    const secondId = await showRelationshipDiagramForConnection('conn-1');

    expect(backendApp.LoadSchema).not.toHaveBeenCalled();
    expect(backendApp.GetSchema).not.toHaveBeenCalled();
    expect(firstId).toBe(secondId);
    expect((get(tabs) as Tab[]).filter((tab) => tab.kind === 'relationshipDiagram')).toHaveLength(1);
    expect(get(activeTabId)).toBe(firstId);
  });
});

describe('activeTabId store', () => {
  it('is set to the first tab id automatically', () => {
    const firstId = (get(tabs) as Tab[])[0].id;
    expect(get(activeTabId)).toBe(firstId);
  });
});

describe('activeTab derived store', () => {
  it('returns the active tab', () => {
    const firstId = (get(tabs) as Tab[])[0].id;
    activeTabId.set(firstId);
    expect((get(activeTab) as any)?.id).toBe(firstId);
  });

  it('returns null when id is invalid', () => {
    activeTabId.set('nonexistent');
    expect(get(activeTab)).toBeNull();
  });
});

describe('activeConnections store', () => {
  it('starts empty', () => {
    expect(get(activeConnections)).toHaveLength(0);
  });
});

describe('statusMessage store', () => {
  it('starts with Ready message', () => {
    expect(get(statusMessage)).toBe('Ready');
  });

  it('can be set', () => {
    statusMessage.set('Connected');
    expect(get(statusMessage)).toBe('Connected');
  });
});

describe('outputTab store', () => {
  it('starts as results', () => {
    expect(get(outputTab)).toBe('results');
  });
});
