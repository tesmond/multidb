// Mock Wails runtime before any store imports
vi.mock('../../wailsjs/go/models', () => ({
  connections: { ConnectionConfig: class {} },
  schema: { SchemaTree: class {} },
  history: { QueryRecord: class {} },
  main: { ExecuteResult: class {} },
}));

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

// Dynamically import to allow mocks to apply first
let tabs: any;
let activeTabId: any;
let activeTab: any;
let activeConnections: any;
let statusMessage: any;
let outputTab: any;

beforeEach(async () => {
  vi.resetModules();
  const mod = await import('./appStore');
  tabs = mod.tabs;
  activeTabId = mod.activeTabId;
  activeTab = mod.activeTab;
  activeConnections = mod.activeConnections;
  statusMessage = mod.statusMessage;
  outputTab = mod.outputTab;
});

describe('tabs store', () => {
  it('starts with one empty tab', () => {
    const $tabs = get(tabs);
    expect($tabs).toHaveLength(1);
    expect($tabs[0].sql).toBe('');
    expect($tabs[0].title).toBe('Query');
  });

  it('add() creates a new tab', () => {
    tabs.add('conn-1');
    expect(get(tabs)).toHaveLength(2);
    expect(get(tabs)[1].connId).toBe('conn-1');
  });

  it('remove() removes the given tab', () => {
    tabs.add('conn-a');
    const id = get(tabs)[0].id;
    tabs.remove(id);
    expect(get(tabs)).not.toContain(expect.objectContaining({ id }));
  });

  it('remove() keeps at least one tab', () => {
    const only = get(tabs)[0].id;
    tabs.remove(only);
    expect(get(tabs)).toHaveLength(1);
  });

  it('updateTab() patches a single tab', () => {
    const id = get(tabs)[0].id;
    tabs.updateTab(id, { sql: 'SELECT 1', running: true });
    const tab = (get(tabs) as any[]).find((t: any) => t.id === id);
    expect(tab?.sql).toBe('SELECT 1');
    expect(tab?.running).toBe(true);
  });
});

describe('activeTabId store', () => {
  it('is set to the first tab id automatically', () => {
    const firstId = get(tabs)[0].id;
    expect(get(activeTabId)).toBe(firstId);
  });
});

describe('activeTab derived store', () => {
  it('returns the active tab', () => {
    const firstId = get(tabs)[0].id;
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
