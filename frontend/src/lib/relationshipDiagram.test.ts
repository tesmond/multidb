import { beforeEach, describe, expect, it } from 'vitest';
import {
  buildRelationshipDiagramQuerySql,
  buildRelationshipDiagramGraph,
  createDefaultRelationshipLayout,
  filterRelationshipDiagramGraph,
  getEffectiveSelectedEdgeId,
  getRelationshipLayoutSchemaHash,
  loadRelationshipLayout,
  mergeRelationshipDiagramLayout,
  projectRelationshipDiagramLayout,
  resetRelationshipLayout,
  saveRelationshipLayout,
} from './relationshipDiagram';

const schemaTree = {
  sizeBytes: 1024,
  tables: [],
  views: [],
  indexes: [],
  schemas: [
    {
      name: 'public',
      sizeBytes: 512,
      tables: [
        {
          name: 'accounts',
          type: 'TABLE',
          columns: [
            { name: 'id', type: 'INTEGER', key: 'PRI', nullable: false, default: '' },
            { name: 'name', type: 'TEXT', key: '', nullable: false, default: '' },
          ],
        },
        {
          name: 'users',
          type: 'TABLE',
          columns: [
            { name: 'id', type: 'INTEGER', key: 'PRI', nullable: false, default: '' },
            { name: 'account_id', type: 'INTEGER', key: '', nullable: false, default: '' },
            { name: 'manager_id', type: 'INTEGER', key: '', nullable: true, default: '' },
          ],
        },
      ],
      views: [],
      indexes: [],
    },
    {
      name: 'audit',
      sizeBytes: 512,
      tables: [
        {
          name: 'events',
          type: 'TABLE',
          columns: [
            { name: 'id', type: 'INTEGER', key: 'PRI', nullable: false, default: '' },
            { name: 'user_id', type: 'INTEGER', key: '', nullable: false, default: '' },
          ],
        },
      ],
      views: [],
      indexes: [],
    },
  ],
  relationships: [
    {
      constraintName: 'users_account_fk',
      sourceTable: { schemaName: 'public', tableName: 'users' },
      targetTable: { schemaName: 'public', tableName: 'accounts' },
      columnPairs: [{ sourceColumn: 'account_id', targetColumn: 'id' }],
      onUpdate: 'CASCADE',
      onDelete: 'RESTRICT',
    },
    {
      constraintName: 'users_manager_fk',
      sourceTable: { schemaName: 'public', tableName: 'users' },
      targetTable: { schemaName: 'public', tableName: 'users' },
      columnPairs: [{ sourceColumn: 'manager_id', targetColumn: 'id' }],
      onUpdate: 'NO ACTION',
      onDelete: 'SET NULL',
    },
    {
      constraintName: 'events_user_fk',
      sourceTable: { schemaName: 'audit', tableName: 'events' },
      targetTable: { schemaName: 'public', tableName: 'users' },
      columnPairs: [{ sourceColumn: 'user_id', targetColumn: 'id' }],
      onUpdate: 'CASCADE',
      onDelete: 'CASCADE',
    },
  ],
};

describe('relationship diagram graph', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('normalizes schema trees into stable table and relationship ids', () => {
    const graph = buildRelationshipDiagramGraph(schemaTree as any);

    expect(graph.tables.map((table) => table.id)).toEqual([
      'audit.events',
      'public.accounts',
      'public.users',
    ]);
    expect(graph.tables.find((table) => table.id === 'public.users')?.columns).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          id: 'public.users.account_id',
          isPrimaryKey: false,
          isForeignKey: true,
        }),
        expect.objectContaining({
          id: 'public.users.id',
          isPrimaryKey: true,
        }),
      ]),
    );
    expect(graph.edges).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          id: 'audit.events::events_user_fk',
          sourceTableId: 'audit.events',
          targetTableId: 'public.users',
          isSelfReferential: false,
          sourceColumnIds: ['audit.events.user_id'],
          targetColumnIds: ['public.users.id'],
        }),
        expect.objectContaining({
          id: 'public.users::users_manager_fk',
          sourceTableId: 'public.users',
          targetTableId: 'public.users',
          isSelfReferential: true,
        }),
      ]),
    );
  });

  it('creates a deterministic default layout', () => {
    const graph = buildRelationshipDiagramGraph(schemaTree as any);

    const first = createDefaultRelationshipLayout(graph);
    const second = createDefaultRelationshipLayout(graph);

    expect(second).toEqual(first);
    expect(first['public.accounts'].x).toBeLessThan(first['public.users'].x);
    expect(first['public.users'].x).toBeLessThan(first['audit.events'].x);
  });

  it('preserves saved positions and auto-places new tables when merging layout', () => {
    const graph = buildRelationshipDiagramGraph(schemaTree as any);
    const defaultLayout = createDefaultRelationshipLayout(graph);

    const merged = mergeRelationshipDiagramLayout(graph, defaultLayout, {
      'public.users': { x: 900, y: 250 },
      'public.accounts': { x: 100, y: 120 },
      'missing.table': { x: 1, y: 1 },
    });

    expect(merged['public.users']).toEqual({ x: 900, y: 250 });
    expect(merged['public.accounts']).toEqual({ x: 100, y: 120 });
    expect(merged['audit.events']).toEqual(defaultLayout['audit.events']);
    expect(merged['missing.table']).toBeUndefined();
  });

  it('filters to matching tables and keeps directly related neighbors visible', () => {
    const graph = buildRelationshipDiagramGraph(schemaTree as any);

    const filtered = filterRelationshipDiagramGraph(graph, 'accounts');

    expect(filtered.tables.map((table) => table.id)).toEqual([
      'public.accounts',
      'public.users',
    ]);
    expect(filtered.edges.map((edge) => edge.id)).toEqual([
      'public.users::users_account_fk',
      'public.users::users_manager_fk',
    ]);
  });

  it('returns the full graph when the filter is empty', () => {
    const graph = buildRelationshipDiagramGraph(schemaTree as any);

    expect(filterRelationshipDiagramGraph(graph, '   ')).toEqual(graph);
  });

  it('projects filtered layouts without mutating saved coordinates', () => {
    const graph = buildRelationshipDiagramGraph(schemaTree as any);
    const filtered = filterRelationshipDiagramGraph(graph, 'accounts');
    const layout = {
      'public.accounts': { x: 100, y: 120 },
      'public.users': { x: 300, y: 200 },
      'audit.events': { x: 540, y: 160 },
    };

    const projected = projectRelationshipDiagramLayout(filtered, layout);

    expect(projected).toEqual({
      'public.accounts': { x: 100, y: 120 },
      'public.users': { x: 300, y: 200 },
    });
    expect(layout['audit.events']).toEqual({ x: 540, y: 160 });
  });

  it('builds quoted query sql for relationship table actions', () => {
    expect(buildRelationshipDiagramQuerySql('postgres', 'public', 'users')).toBe(
      'SELECT * FROM "public"."users" LIMIT 100;',
    );
    expect(buildRelationshipDiagramQuerySql('mysql', 'app', 'order-items')).toBe(
      'SELECT * FROM `app`.`order-items` LIMIT 100;',
    );
    expect(buildRelationshipDiagramQuerySql('sqlite', '', 'users')).toBe(
      'SELECT * FROM "users" LIMIT 100;',
    );
  });

  it('falls back to the first visible edge when the selected edge disappears', () => {
    const graph = buildRelationshipDiagramGraph(schemaTree as any);
    const filtered = filterRelationshipDiagramGraph(graph, 'accounts');

    expect(getEffectiveSelectedEdgeId(filtered.edges, 'missing-edge')).toBe(
      'public.users::users_account_fk',
    );
    expect(getEffectiveSelectedEdgeId(filtered.edges, 'public.users::users_manager_fk')).toBe(
      'public.users::users_manager_fk',
    );
  });

  it('handles larger schemas with stable output shapes', () => {
    const largeSchema = {
      sizeBytes: 0,
      tables: [],
      views: [],
      indexes: [],
      schemas: [
        {
          name: 'public',
          sizeBytes: 0,
          tables: Array.from({ length: 150 }, (_, index) => ({
            name: `table_${index}`,
            type: 'TABLE',
            columns: [
              { name: 'id', type: 'INTEGER', key: 'PRI', nullable: false, default: '' },
              { name: 'parent_id', type: 'INTEGER', key: '', nullable: true, default: '' },
            ],
          })),
          views: [],
          indexes: [],
        },
      ],
      relationships: Array.from({ length: 149 }, (_, index) => ({
        constraintName: `fk_${index}`,
        sourceTable: { schemaName: 'public', tableName: `table_${index + 1}` },
        targetTable: { schemaName: 'public', tableName: `table_${index}` },
        columnPairs: [{ sourceColumn: 'parent_id', targetColumn: 'id' }],
        onUpdate: 'CASCADE',
        onDelete: 'SET NULL',
      })),
    };

    const graph = buildRelationshipDiagramGraph(largeSchema as any);
    const layout = createDefaultRelationshipLayout(graph);

    expect(graph.tables).toHaveLength(150);
    expect(graph.edges).toHaveLength(149);
    expect(Object.keys(layout)).toHaveLength(150);
  });

  it('persists, loads, and resets layouts by connection id and schema hash', () => {
    const schemaHash = getRelationshipLayoutSchemaHash(schemaTree as any);
    const layout = {
      'public.users': { x: 320, y: 180 },
    };

    saveRelationshipLayout('conn-1', schemaHash, layout);

    expect(loadRelationshipLayout('conn-1', schemaHash)).toEqual(layout);
    expect(loadRelationshipLayout('conn-2', schemaHash)).toBeNull();

    resetRelationshipLayout('conn-1', schemaHash);

    expect(loadRelationshipLayout('conn-1', schemaHash)).toBeNull();
  });
});