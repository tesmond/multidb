import type { schema } from '../../desktop/gen/models';

export type SchemaTree = schema.SchemaTree;
export type Relationship = schema.Relationship;

export interface DiagramColumnNode {
  id: string;
  name: string;
  type: string;
  key: string;
  isPrimaryKey: boolean;
  isForeignKey: boolean;
}

export interface DiagramTableNode {
  id: string;
  schemaName: string;
  tableName: string;
  title: string;
  width: number;
  height: number;
  columns: DiagramColumnNode[];
}

export interface DiagramRelationshipEdge {
  id: string;
  constraintName: string;
  sourceTableId: string;
  targetTableId: string;
  sourceColumnIds: string[];
  targetColumnIds: string[];
  onUpdate: string;
  onDelete: string;
  isSelfReferential: boolean;
}

export interface DiagramGraph {
  tables: DiagramTableNode[];
  edges: DiagramRelationshipEdge[];
}

export interface DiagramPoint {
  x: number;
  y: number;
}

export type DiagramLayout = Record<string, DiagramPoint>;

const STORAGE_KEY = 'multidb.relationshipLayouts.v1';
const HEADER_HEIGHT = 40;
const ROW_HEIGHT = 24;
const MIN_WIDTH = 220;
const WIDTH_PER_CHAR = 7;
const COLUMN_PADDING = 56;
const CLUSTER_GAP_X = 180;
const LAYER_GAP_X = 280;
const NODE_GAP_Y = 48;

export function buildRelationshipDiagramGraph(schemaTree: SchemaTree): DiagramGraph {
  const foreignKeyColumns = collectForeignKeyColumns(schemaTree.relationships ?? []);
  const tables = collectTables(schemaTree)
    .map(({ schemaName, table }) => makeTableNode(schemaName, table, foreignKeyColumns))
    .sort((left, right) => left.id.localeCompare(right.id));

  const edges = (schemaTree.relationships ?? [])
    .map((relationship) => makeRelationshipEdge(relationship))
    .sort((left, right) => left.id.localeCompare(right.id));

  return { tables, edges };
}

export function createDefaultRelationshipLayout(graph: DiagramGraph): DiagramLayout {
  const tableIds = graph.tables.map((table) => table.id);
  const tableById = new Map(graph.tables.map((table) => [table.id, table]));
  const undirected = new Map<string, Set<string>>();
  const parentToChildren = new Map<string, Set<string>>();
  const indegree = new Map<string, number>();

  for (const tableId of tableIds) {
    undirected.set(tableId, new Set());
    parentToChildren.set(tableId, new Set());
    indegree.set(tableId, 0);
  }

  for (const edge of graph.edges) {
    undirected.get(edge.sourceTableId)?.add(edge.targetTableId);
    undirected.get(edge.targetTableId)?.add(edge.sourceTableId);
    if (edge.isSelfReferential) continue;

    parentToChildren.get(edge.targetTableId)?.add(edge.sourceTableId);
    indegree.set(
      edge.sourceTableId,
      (indegree.get(edge.sourceTableId) ?? 0) + 1,
    );
  }

  const components = collectComponents(tableIds, undirected);
  const layout: DiagramLayout = {};
  let clusterX = 0;

  for (const component of components) {
    const componentSet = new Set(component);
    const componentParents = new Map<string, Set<string>>();
    const componentChildren = new Map<string, Set<string>>();
    const componentIndegree = new Map<string, number>();

    for (const tableId of component) {
      componentParents.set(tableId, new Set());
      componentChildren.set(tableId, new Set());
      componentIndegree.set(tableId, 0);
    }

    for (const parentId of component) {
      for (const childId of parentToChildren.get(parentId) ?? []) {
        if (!componentSet.has(childId)) continue;
        componentChildren.get(parentId)?.add(childId);
        componentParents.get(childId)?.add(parentId);
        componentIndegree.set(childId, (componentIndegree.get(childId) ?? 0) + 1);
      }
    }

    const layers = assignLayers(component, componentChildren, componentParents, componentIndegree);
    const layerIds = Array.from(new Set(component.map((tableId) => layers.get(tableId) ?? 0))).sort((a, b) => a - b);
    let componentWidth = 0;

    for (const layerId of layerIds) {
      const layerTables = component
        .filter((tableId) => (layers.get(tableId) ?? 0) === layerId)
        .sort((left, right) => left.localeCompare(right));

      let currentY = 0;
      let widestTable = 0;
      for (const tableId of layerTables) {
        const table = tableById.get(tableId);
        if (!table) continue;

        layout[tableId] = {
          x: clusterX + layerId * LAYER_GAP_X,
          y: currentY,
        };
        widestTable = Math.max(widestTable, table.width);
        currentY += table.height + NODE_GAP_Y;
      }

      componentWidth = Math.max(componentWidth, layerId * LAYER_GAP_X + widestTable);
    }

    clusterX += componentWidth + CLUSTER_GAP_X;
  }

  return layout;
}

export function filterRelationshipDiagramGraph(
  graph: DiagramGraph,
  filterText: string,
): DiagramGraph {
  const normalized = filterText.trim().toLowerCase();
  if (!normalized) return graph;

  const matchingTableIds = new Set(
    graph.tables
      .filter((table) =>
        table.title.toLowerCase().includes(normalized) ||
        table.tableName.toLowerCase().includes(normalized) ||
        table.columns.some((column) => column.name.toLowerCase().includes(normalized)),
      )
      .map((table) => table.id),
  );

  const visibleTableIds = new Set(matchingTableIds);
  for (const edge of graph.edges) {
    if (matchingTableIds.has(edge.sourceTableId) || matchingTableIds.has(edge.targetTableId)) {
      visibleTableIds.add(edge.sourceTableId);
      visibleTableIds.add(edge.targetTableId);
    }
  }

  return {
    tables: graph.tables.filter((table) => visibleTableIds.has(table.id)),
    edges: graph.edges.filter(
      (edge) => visibleTableIds.has(edge.sourceTableId) && visibleTableIds.has(edge.targetTableId),
    ),
  };
}

export function projectRelationshipDiagramLayout(
  graph: DiagramGraph,
  layout: DiagramLayout,
): DiagramLayout {
  const projected: DiagramLayout = {};
  for (const table of graph.tables) {
    if (layout[table.id]) {
      projected[table.id] = layout[table.id];
    }
  }
  return projected;
}

export function buildRelationshipDiagramQuerySql(
  driver: string,
  schemaName: string,
  tableName: string,
): string {
  const quotedTable = quoteIdentifier(driver, tableName);
  const qualifiedName = schemaName
    ? `${quoteIdentifier(driver, schemaName)}.${quotedTable}`
    : quotedTable;
  return `SELECT * FROM ${qualifiedName} LIMIT 100;`;
}

export function getEffectiveSelectedEdgeId(
  edges: DiagramRelationshipEdge[],
  selectedEdgeId: string,
): string {
  return edges.some((edge) => edge.id === selectedEdgeId)
    ? selectedEdgeId
    : (edges[0]?.id ?? '');
}

export function mergeRelationshipDiagramLayout(
  graph: DiagramGraph,
  defaultLayout: DiagramLayout,
  persistedLayout: DiagramLayout | null,
): DiagramLayout {
  const merged: DiagramLayout = {};

  for (const table of graph.tables) {
    merged[table.id] = persistedLayout?.[table.id] ?? defaultLayout[table.id] ?? { x: 0, y: 0 };
  }

  return merged;
}

export function getRelationshipLayoutSchemaHash(schemaTree: SchemaTree): string {
  const canonical = stableStringify(schemaTree);
  let hash = 2166136261;
  for (let index = 0; index < canonical.length; index += 1) {
    hash ^= canonical.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return (hash >>> 0).toString(16).padStart(8, '0');
}

export function loadRelationshipLayout(
  connId: string,
  schemaHash: string,
): DiagramLayout | null {
  if (typeof localStorage === 'undefined') return null;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Record<string, DiagramLayout>;
    return parsed[makePersistedLayoutId(connId, schemaHash)] ?? null;
  } catch {
    return null;
  }
}

export function saveRelationshipLayout(
  connId: string,
  schemaHash: string,
  layout: DiagramLayout,
): void {
  if (typeof localStorage === 'undefined') return;
  const nextLayouts = readPersistedLayouts();
  nextLayouts[makePersistedLayoutId(connId, schemaHash)] = layout;
  localStorage.setItem(STORAGE_KEY, JSON.stringify(nextLayouts));
}

export function resetRelationshipLayout(connId: string, schemaHash: string): void {
  if (typeof localStorage === 'undefined') return;
  const nextLayouts = readPersistedLayouts();
  delete nextLayouts[makePersistedLayoutId(connId, schemaHash)];
  localStorage.setItem(STORAGE_KEY, JSON.stringify(nextLayouts));
}

function collectTables(schemaTree: SchemaTree): Array<{ schemaName: string; table: schema.Table }> {
  if (schemaTree.schemas?.length) {
    return schemaTree.schemas.flatMap((dbSchema) =>
      (dbSchema.tables ?? []).map((table) => ({ schemaName: dbSchema.name ?? '', table })),
    );
  }

  return (schemaTree.tables ?? []).map((table) => ({ schemaName: '', table }));
}

function collectForeignKeyColumns(relationships: Relationship[]): Set<string> {
  const columns = new Set<string>();
  for (const relationship of relationships) {
    const tableId = makeTableId(
      relationship.sourceTable?.schemaName ?? '',
      relationship.sourceTable?.tableName ?? '',
    );
    for (const pair of relationship.columnPairs ?? []) {
      columns.add(makeColumnId(tableId, pair.sourceColumn ?? ''));
    }
  }
  return columns;
}

function makeTableNode(
  schemaName: string,
  table: schema.Table,
  foreignKeyColumns: Set<string>,
): DiagramTableNode {
  const tableId = makeTableId(schemaName, table.name ?? '');
  const columns = (table.columns ?? []).map((column) => {
    const columnId = makeColumnId(tableId, column.name ?? '');
    return {
      id: columnId,
      name: column.name ?? '',
      type: column.type ?? '',
      key: column.key ?? '',
      isPrimaryKey: (column.key ?? '') === 'PRI',
      isForeignKey: foreignKeyColumns.has(columnId),
    };
  });

  return {
    id: tableId,
    schemaName,
    tableName: table.name ?? '',
    title: schemaName ? `${schemaName}.${table.name ?? ''}` : table.name ?? '',
    width: estimateTableWidth(schemaName, table.name ?? '', columns),
    height: HEADER_HEIGHT + columns.length * ROW_HEIGHT,
    columns,
  };
}

function makeRelationshipEdge(relationship: Relationship): DiagramRelationshipEdge {
  const sourceTableId = makeTableId(
    relationship.sourceTable?.schemaName ?? '',
    relationship.sourceTable?.tableName ?? '',
  );
  const targetTableId = makeTableId(
    relationship.targetTable?.schemaName ?? '',
    relationship.targetTable?.tableName ?? '',
  );

  return {
    id: `${sourceTableId}::${relationship.constraintName ?? ''}`,
    constraintName: relationship.constraintName ?? '',
    sourceTableId,
    targetTableId,
    sourceColumnIds: (relationship.columnPairs ?? []).map((pair) =>
      makeColumnId(sourceTableId, pair.sourceColumn ?? ''),
    ),
    targetColumnIds: (relationship.columnPairs ?? []).map((pair) =>
      makeColumnId(targetTableId, pair.targetColumn ?? ''),
    ),
    onUpdate: relationship.onUpdate ?? '',
    onDelete: relationship.onDelete ?? '',
    isSelfReferential: sourceTableId === targetTableId,
  };
}

function estimateTableWidth(
  schemaName: string,
  tableName: string,
  columns: DiagramColumnNode[],
): number {
  const headerText = schemaName ? `${schemaName}.${tableName}` : tableName;
  const widestLine = columns.reduce(
    (maxWidth, column) => Math.max(maxWidth, `${column.name}: ${column.type}`.length),
    headerText.length,
  );
  return Math.max(MIN_WIDTH, widestLine * WIDTH_PER_CHAR + COLUMN_PADDING);
}

function collectComponents(
  tableIds: string[],
  adjacency: Map<string, Set<string>>,
): string[][] {
  const visited = new Set<string>();
  const components: string[][] = [];
  const sortedTableIds = [...tableIds].sort((left, right) => left.localeCompare(right));

  for (const tableId of sortedTableIds) {
    if (visited.has(tableId)) continue;
    const stack = [tableId];
    const component: string[] = [];
    visited.add(tableId);

    while (stack.length > 0) {
      const current = stack.pop()!;
      component.push(current);
      const neighbors = [...(adjacency.get(current) ?? [])].sort((left, right) =>
        left.localeCompare(right),
      );
      for (const neighbor of neighbors) {
        if (visited.has(neighbor)) continue;
        visited.add(neighbor);
        stack.push(neighbor);
      }
    }

    components.push(component.sort((left, right) => left.localeCompare(right)));
  }

  return components.sort((left, right) => left[0].localeCompare(right[0]));
}

function assignLayers(
  component: string[],
  childrenByParent: Map<string, Set<string>>,
  parentsByChild: Map<string, Set<string>>,
  indegree: Map<string, number>,
): Map<string, number> {
  const queue = component
    .filter((tableId) => (indegree.get(tableId) ?? 0) === 0)
    .sort((left, right) => left.localeCompare(right));
  const remainingIndegree = new Map(indegree);
  const layers = new Map<string, number>();
  const seen = new Set<string>();

  while (queue.length > 0) {
    const current = queue.shift()!;
    seen.add(current);
    const parentLayers = [...(parentsByChild.get(current) ?? [])]
      .map((parentId) => layers.get(parentId) ?? 0);
    const currentLayer = parentLayers.length > 0 ? Math.max(...parentLayers) + 1 : 0;
    layers.set(current, currentLayer);

    const children = [...(childrenByParent.get(current) ?? [])].sort((left, right) =>
      left.localeCompare(right),
    );
    for (const childId of children) {
      const nextIndegree = (remainingIndegree.get(childId) ?? 0) - 1;
      remainingIndegree.set(childId, nextIndegree);
      if (nextIndegree === 0) queue.push(childId);
    }
    queue.sort((left, right) => left.localeCompare(right));
  }

  for (const tableId of [...component].sort((left, right) => left.localeCompare(right))) {
    if (seen.has(tableId)) continue;
    const parentLayers = [...(parentsByChild.get(tableId) ?? [])]
      .map((parentId) => layers.get(parentId))
      .filter((value): value is number => value !== undefined);
    layers.set(tableId, parentLayers.length > 0 ? Math.max(...parentLayers) + 1 : 0);
  }

  return layers;
}

function makePersistedLayoutId(connId: string, schemaHash: string): string {
  return `${connId}::${schemaHash}`;
}

function quoteIdentifier(driver: string, name: string): string {
  if (driver === 'mysql') return `\`${name.replace(/`/g, '``')}\``;
  return `"${name.replace(/"/g, '""')}"`;
}

function readPersistedLayouts(): Record<string, DiagramLayout> {
  if (typeof localStorage === 'undefined') return {};
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === 'object' ? parsed : {};
  } catch {
    return {};
  }
}

function stableStringify(value: unknown): string {
  if (value === null || value === undefined) return 'null';
  if (typeof value === 'number' || typeof value === 'boolean') return JSON.stringify(value);
  if (typeof value === 'string') return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map((entry) => stableStringify(entry)).join(',')}]`;

  const entries = Object.entries(value as Record<string, unknown>)
    .filter(([, entry]) => entry !== undefined)
    .sort(([left], [right]) => left.localeCompare(right));

  return `{${entries
    .map(([key, entry]) => `${JSON.stringify(key)}:${stableStringify(entry)}`)
    .join(',')}}`;
}

function makeTableId(schemaName: string, tableName: string): string {
  return schemaName ? `${schemaName}.${tableName}` : tableName;
}

function makeColumnId(tableId: string, columnName: string): string {
  return `${tableId}.${columnName}`;
}