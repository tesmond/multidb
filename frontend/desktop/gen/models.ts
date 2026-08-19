export namespace connections {
	
	export class ConnectionConfig {
	    id: string;
	    name: string;
	    driver: string;
	    tabColor: string;
	    tabTextBlack: boolean;
	    host: string;
	    port: number;
	    username: string;
	    password: string;
	    hasSavedPassword: boolean;
	    authMode: string;
	    database: string;
	    dsn: string;
	    awsRegion: string;
	    awsProfile: string;
	    sslCaPath: string;
	    useKubePortForward: boolean;
	    kubeContext: string;
	    kubeNamespace: string;
	    kubeResource: string;
	    kubeLocalPort: number;
	    kubeRemotePort: number;
	
	    static createFrom(source: any = {}) {
	        return new ConnectionConfig(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.id = source["id"];
	        this.name = source["name"];
	        this.driver = source["driver"];
	        this.tabColor = source["tabColor"];
	        this.tabTextBlack = source["tabTextBlack"];
	        this.host = source["host"];
	        this.port = source["port"];
	        this.username = source["username"];
	        this.password = source["password"];
	        this.hasSavedPassword = source["hasSavedPassword"];
	        this.authMode = source["authMode"];
	        this.database = source["database"];
	        this.dsn = source["dsn"];
	        this.awsRegion = source["awsRegion"];
	        this.awsProfile = source["awsProfile"];
	        this.sslCaPath = source["sslCaPath"];
	        this.useKubePortForward = source["useKubePortForward"];
	        this.kubeContext = source["kubeContext"];
	        this.kubeNamespace = source["kubeNamespace"];
	        this.kubeResource = source["kubeResource"];
	        this.kubeLocalPort = source["kubeLocalPort"];
	        this.kubeRemotePort = source["kubeRemotePort"];
	    }
	}

}

export namespace history {
	
	export class QueryRecord {
	    id: number;
	    connId: string;
	    query: string;
	    duration: number;
	    resultCount: number;
	    error?: string;
	    createdAt: string;
	
	    static createFrom(source: any = {}) {
	        return new QueryRecord(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.id = source["id"];
	        this.connId = source["connId"];
	        this.query = source["query"];
	        this.duration = source["duration"];
	        this.resultCount = source["resultCount"];
	        this.error = source["error"];
	        this.createdAt = source["createdAt"];
	    }
	}
	export class SavedQuery {
	    id: number;
	    connId: string;
	    title: string;
	    query: string;
	    createdAt: string;
	
	    static createFrom(source: any = {}) {
	        return new SavedQuery(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.id = source["id"];
	        this.connId = source["connId"];
	        this.title = source["title"];
	        this.query = source["query"];
	        this.createdAt = source["createdAt"];
	    }
	}

}

export namespace main {
	
	export class DatabaseConnection {
	    id: string;
	    user: string;
	    database: string;
	    client: string;
	    state: string;
	    openedAt: string;
	    lastActiveAt: string;
	    mostRecentCommand: string;
	    canTerminate: boolean;
	
	    static createFrom(source: any = {}) {
	        return new DatabaseConnection(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.id = source["id"];
	        this.user = source["user"];
	        this.database = source["database"];
	        this.client = source["client"];
	        this.state = source["state"];
	        this.openedAt = source["openedAt"];
	        this.lastActiveAt = source["lastActiveAt"];
	        this.mostRecentCommand = source["mostRecentCommand"];
	        this.canTerminate = source["canTerminate"];
	    }
	}
	
	export class ExecuteResult {
	    columns: string[];
	    columnTypes: string[];
	    rows: any[][];
	    rowsAffected: number;
	    duration: number;
	    error?: string;
	
	    static createFrom(source: any = {}) {
	        return new ExecuteResult(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.columns = source["columns"];
	        this.columnTypes = source["columnTypes"];
	        this.rows = source["rows"];
	        this.rowsAffected = source["rowsAffected"];
	        this.duration = source["duration"];
	        this.error = source["error"];
	    }
	}
	export class SchemaCacheEntry {
	    schemaJson: string;
	    lastRefreshedAt: string;
	    hash: string;
	
	    static createFrom(source: any = {}) {
	        return new SchemaCacheEntry(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.schemaJson = source["schemaJson"];
	        this.lastRefreshedAt = source["lastRefreshedAt"];
	        this.hash = source["hash"];
	    }
	}

}

export namespace schema {
	
	export class Column {
	    name: string;
	    type: string;
	    nullable: boolean;
	    default: string;
	    key: string;
	
	    static createFrom(source: any = {}) {
	        return new Column(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.name = source["name"];
	        this.type = source["type"];
	        this.nullable = source["nullable"];
	        this.default = source["default"];
	        this.key = source["key"];
	    }
	}
	export class RelationshipTableRef {
	    schemaName?: string;
	    tableName: string;
	
	    static createFrom(source: any = {}) {
	        return new RelationshipTableRef(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.schemaName = source["schemaName"];
	        this.tableName = source["tableName"];
	    }
	}
	export class RelationshipColumnPair {
	    sourceColumn: string;
	    targetColumn: string;
	
	    static createFrom(source: any = {}) {
	        return new RelationshipColumnPair(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.sourceColumn = source["sourceColumn"];
	        this.targetColumn = source["targetColumn"];
	    }
	}
	export class Relationship {
	    constraintName: string;
	    sourceTable: RelationshipTableRef;
	    targetTable: RelationshipTableRef;
	    columnPairs?: RelationshipColumnPair[];
	    onUpdate?: string;
	    onDelete?: string;
	
	    static createFrom(source: any = {}) {
	        return new Relationship(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.constraintName = source["constraintName"];
	        this.sourceTable = this.convertValues(source["sourceTable"], RelationshipTableRef);
	        this.targetTable = this.convertValues(source["targetTable"], RelationshipTableRef);
	        this.columnPairs = this.convertValues(source["columnPairs"], RelationshipColumnPair);
	        this.onUpdate = source["onUpdate"];
	        this.onDelete = source["onDelete"];
	    }
	
		convertValues(a: any, classs: any, asMap: boolean = false): any {
		    if (!a) {
		        return a;
		    }
		    if (a.slice && a.map) {
		        return (a as any[]).map(elem => this.convertValues(elem, classs));
		    } else if ("object" === typeof a) {
		        if (asMap) {
		            for (const key of Object.keys(a)) {
		                a[key] = new classs(a[key]);
		            }
		            return a;
		        }
		        return new classs(a);
		    }
		    return a;
		}
	}
	export class Table {
	    name: string;
	    type: string;
	    sizeBytes?: number;
	    columns?: Column[];
	
	    static createFrom(source: any = {}) {
	        return new Table(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.name = source["name"];
	        this.type = source["type"];
	        this.sizeBytes = source["sizeBytes"];
	        this.columns = this.convertValues(source["columns"], Column);
	    }
	
		convertValues(a: any, classs: any, asMap: boolean = false): any {
		    if (!a) {
		        return a;
		    }
		    if (a.slice && a.map) {
		        return (a as any[]).map(elem => this.convertValues(elem, classs));
		    } else if ("object" === typeof a) {
		        if (asMap) {
		            for (const key of Object.keys(a)) {
		                a[key] = new classs(a[key]);
		            }
		            return a;
		        }
		        return new classs(a);
		    }
		    return a;
		}
	}
	export class Schema {
	    name: string;
	    sizeBytes?: number;
	    tables: Table[];
	    views: Table[];
	    indexes: string[];
	
	    static createFrom(source: any = {}) {
	        return new Schema(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.name = source["name"];
	        this.sizeBytes = source["sizeBytes"];
	        this.tables = this.convertValues(source["tables"], Table);
	        this.views = this.convertValues(source["views"], Table);
	        this.indexes = source["indexes"];
	    }
	
		convertValues(a: any, classs: any, asMap: boolean = false): any {
		    if (!a) {
		        return a;
		    }
		    if (a.slice && a.map) {
		        return (a as any[]).map(elem => this.convertValues(elem, classs));
		    } else if ("object" === typeof a) {
		        if (asMap) {
		            for (const key of Object.keys(a)) {
		                a[key] = new classs(a[key]);
		            }
		            return a;
		        }
		        return new classs(a);
		    }
		    return a;
		}
	}
	export class SchemaTree {
	    sizeBytes?: number;
	    tables: Table[];
	    views: Table[];
	    indexes: string[];
	    relationships?: Relationship[];
	    schemas?: Schema[];
	
	    static createFrom(source: any = {}) {
	        return new SchemaTree(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.sizeBytes = source["sizeBytes"];
	        this.tables = this.convertValues(source["tables"], Table);
	        this.views = this.convertValues(source["views"], Table);
	        this.indexes = source["indexes"];
	        this.relationships = this.convertValues(source["relationships"], Relationship);
	        this.schemas = this.convertValues(source["schemas"], Schema);
	    }
	
		convertValues(a: any, classs: any, asMap: boolean = false): any {
		    if (!a) {
		        return a;
		    }
		    if (a.slice && a.map) {
		        return (a as any[]).map(elem => this.convertValues(elem, classs));
		    } else if ("object" === typeof a) {
		        if (asMap) {
		            for (const key of Object.keys(a)) {
		                a[key] = new classs(a[key]);
		            }
		            return a;
		        }
		        return new classs(a);
		    }
		    return a;
		}
	}

}
