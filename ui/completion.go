package ui

import (
	"strings"
	"unicode"

	"multidb/backend/schema"
)

// CompletionItem is a single completion suggestion.
type CompletionItem struct {
	Label  string
	Type   string // "table" | "column" | "schema" | "keyword"
	Detail string
}

// SchemaForCompletion is extracted from an ActiveConn schema.
type SchemaForCompletion struct {
	Driver  string
	Schemas []schema.Schema // for postgres/mysql (multi-schema)
	Tables  []schema.Table  // for sqlite/flat
	Views   []schema.Table
}

// BuildSchemaForCompletion converts an ActiveConn schema tree into the
// completion-friendly format.
func BuildSchemaForCompletion(conn *ActiveConn) *SchemaForCompletion {
	if conn == nil || conn.Schema == nil {
		return nil
	}
	return &SchemaForCompletion{
		Driver:  conn.Config.Driver,
		Schemas: conn.Schema.Schemas,
		Tables:  conn.Schema.Tables,
		Views:   conn.Schema.Views,
	}
}

var sqlKeywords = []string{
	"SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "IN", "IS", "NULL",
	"ORDER", "BY", "GROUP", "HAVING", "LIMIT", "OFFSET", "DISTINCT",
	"INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE",
	"JOIN", "INNER", "LEFT", "RIGHT", "OUTER", "CROSS", "ON",
	"CREATE", "TABLE", "INDEX", "VIEW", "DROP", "ALTER",
	"AS", "CASE", "WHEN", "THEN", "ELSE", "END",
	"COUNT", "SUM", "AVG", "MIN", "MAX", "COALESCE", "NULLIF",
	"UNION", "ALL", "EXCEPT", "INTERSECT", "WITH", "RECURSIVE",
	"PRIMARY", "KEY", "FOREIGN", "REFERENCES", "UNIQUE", "DEFAULT",
	"ASC", "DESC", "LIKE", "BETWEEN", "EXISTS", "ANY",
}

var reservedWords = map[string]bool{
	"WHERE": true, "ON": true, "SET": true, "GROUP": true, "ORDER": true,
	"HAVING": true, "LIMIT": true, "OFFSET": true, "INNER": true,
	"LEFT": true, "RIGHT": true, "OUTER": true, "CROSS": true,
	"NATURAL": true, "FULL": true, "SELECT": true, "FROM": true,
	"JOIN": true, "INTO": true, "VALUES": true, "UPDATE": true,
	"DELETE": true, "INSERT": true, "CREATE": true, "DROP": true,
	"ALTER": true, "TABLE": true, "INDEX": true, "VIEW": true,
	"AS": true, "BY": true, "AND": true, "OR": true, "NOT": true,
	"IN": true, "IS": true, "NULL": true, "LIKE": true,
	"BETWEEN": true, "EXISTS": true, "CASE": true, "WHEN": true,
	"THEN": true, "ELSE": true, "END": true, "DISTINCT": true,
	"ALL": true, "ANY": true, "UNION": true, "INTERSECT": true,
	"EXCEPT": true, "WITH": true, "RECURSIVE": true, "USING": true,
	"TRUE": true, "FALSE": true, "PRIMARY": true, "FOREIGN": true,
	"KEY": true, "UNIQUE": true, "CONSTRAINT": true, "DEFAULT": true,
	"CHECK": true, "REFERENCES": true, "RETURNING": true,
}
// GetCompletions returns completion items for text[0:cursorPos].
func GetCompletions(db *SchemaForCompletion, text string, cursorPos int) []CompletionItem {
	if db == nil || cursorPos > len(text) {
		return nil
	}
	before := text[:cursorPos]

	// --- Three-part: schema.table.partial ---
	if schemaName, tableName, partial, ok := parseThreePart(before); ok {
		return threePartCompletions(db, schemaName, tableName, partial)
	}

	// --- Two-part: prefix.partial ---
	if prefix, partial, ok := parseTwoPart(before); ok {
		return twoPartCompletions(db, text, prefix, partial)
	}

	// --- Single word ---
	word := wordBefore(before)
	return topLevelCompletions(db, text, word)
}

func parseThreePart(before string) (schemaName, tableName, partial string, ok bool) {
	// Match word.word.partial at end
	i := len(before)
	// collect partial after last dot
	start := i
	for start > 0 && isWordChar(rune(before[start-1])) {
		start--
	}
	if start == 0 || before[start-1] != '.'  {
		return "", "", "", false
	}
	partial = before[start:]
	rest := before[:start-1]
	// collect table name
	tStart := len(rest)
	for tStart > 0 && isWordChar(rune(rest[tStart-1])) {
		tStart--
	}
	if tStart == 0 || rest[tStart-1] != '.'  {
		return "", "", "", false
	}
	tableName = rest[tStart:]
	schemaName = ""
	// collect schema name
	sRest := rest[:tStart-1]
	sStart := len(sRest)
	for sStart > 0 && isWordChar(rune(sRest[sStart-1])) {
		sStart--
	}
	schemaName = sRest[sStart:]
	if schemaName == "" {
		return "", "", "", false
	}
	return schemaName, tableName, partial, true
}

func parseTwoPart(before string) (prefix, partial string, ok bool) {
	i := len(before)
	start := i
	for start > 0 && isWordChar(rune(before[start-1])) {
		start--
	}
	if start == 0 || before[start-1] != '.'  {
		return "", "", false
	}
	partial = before[start:]
	rest := before[:start-1]
	pStart := len(rest)
	for pStart > 0 && isWordChar(rune(rest[pStart-1])) {
		pStart--
	}
	prefix = rest[pStart:]
	if prefix == "" {
		return "", "", false
	}
	return prefix, partial, true
}

func wordBefore(before string) string {
	i := len(before)
	for i > 0 && isWordChar(rune(before[i-1])) {
		i--
	}
	return before[i:]
}

func isWordChar(r rune) bool {
	return unicode.IsLetter(r) || unicode.IsDigit(r) || r == '_'
}

func threePartCompletions(db *SchemaForCompletion, schemaName, tableName, partial string) []CompletionItem {
	for _, s := range db.Schemas {
		if strings.EqualFold(s.Name, schemaName) {
			for _, t := range s.Tables {
				if strings.EqualFold(t.Name, tableName) {
					return filterColumns(t.Columns, partial)
				}
			}
		}
	}
	return nil
}

func twoPartCompletions(db *SchemaForCompletion, fullText, prefix, partial string) []CompletionItem {
	lower := strings.ToLower(prefix)

	// Check schema name -> tables
	for _, s := range db.Schemas {
		if strings.EqualFold(s.Name, prefix) {
			var items []CompletionItem
			for _, t := range s.Tables {
				if strings.HasPrefix(strings.ToLower(t.Name), strings.ToLower(partial)) {
					items = append(items, CompletionItem{Label: t.Name, Type: "table"})
				}
			}
			for _, v := range s.Views {
				if strings.HasPrefix(strings.ToLower(v.Name), strings.ToLower(partial)) {
					items = append(items, CompletionItem{Label: v.Name, Type: "view"})
				}
			}
			return items
		}
	}

	// Check table / alias -> columns
	aliases := extractAliases(fullText, db)
	if cols, ok := aliases[lower]; ok {
		return filterColumns(cols, partial)
	}
	if cols, ok := aliases[prefix]; ok {
		return filterColumns(cols, partial)
	}

	// Flat tables
	for _, t := range db.Tables {
		if strings.EqualFold(t.Name, prefix) {
			return filterColumns(t.Columns, partial)
		}
	}
	return nil
}
func topLevelCompletions(db *SchemaForCompletion, fullText, word string) []CompletionItem {
	lower := strings.ToLower(word)
	before := strings.ToUpper(fullText)
	inFrom := strings.Contains(before, "FROM") || strings.Contains(before, "JOIN")
	inColCtx := strings.Contains(before, "SELECT") || strings.Contains(before, "WHERE") ||
		strings.Contains(before, "SET") || strings.Contains(before, "ON") ||
		strings.Contains(before, "HAVING")

	seen := map[string]bool{}
	var items []CompletionItem

	add := func(item CompletionItem) {
		if !seen[item.Label] {
			seen[item.Label] = true
			items = append(items, item)
		}
	}

	// Keywords
	for _, kw := range sqlKeywords {
		if lower == "" || strings.HasPrefix(strings.ToLower(kw), lower) {
			add(CompletionItem{Label: kw, Type: "keyword"})
		}
	}

	// Schemas
	for _, s := range db.Schemas {
		if lower == "" || strings.HasPrefix(strings.ToLower(s.Name), lower) {
			add(CompletionItem{Label: s.Name, Type: "schema"})
		}
		// Tables within schema
		for _, t := range s.Tables {
			if lower == "" || strings.HasPrefix(strings.ToLower(t.Name), lower) {
				detail := ""
				if inFrom {
					detail = s.Name + "."
				}
				add(CompletionItem{Label: t.Name, Type: "table", Detail: detail + t.Name})
			}
			if inColCtx {
				for _, c := range t.Columns {
					if lower == "" || strings.HasPrefix(strings.ToLower(c.Name), lower) {
						detail := c.Type
						if c.Key == "PRI" {
							detail += " PK"
						}
						add(CompletionItem{Label: c.Name, Type: "column", Detail: strings.TrimSpace(detail)})
					}
				}
			}
		}
	}

	// Flat tables (sqlite)
	for _, t := range db.Tables {
		if lower == "" || strings.HasPrefix(strings.ToLower(t.Name), lower) {
			add(CompletionItem{Label: t.Name, Type: "table"})
		}
		if inColCtx {
			for _, c := range t.Columns {
				if lower == "" || strings.HasPrefix(strings.ToLower(c.Name), lower) {
					add(CompletionItem{Label: c.Name, Type: "column", Detail: c.Type})
				}
			}
		}
	}
	for _, v := range db.Views {
		if lower == "" || strings.HasPrefix(strings.ToLower(v.Name), lower) {
			add(CompletionItem{Label: v.Name, Type: "view"})
		}
	}

	return items
}

func filterColumns(cols []schema.Column, partial string) []CompletionItem {
	var items []CompletionItem
	lower := strings.ToLower(partial)
	for _, c := range cols {
		if partial == "" || strings.HasPrefix(strings.ToLower(c.Name), lower) {
			detail := c.Type
			if c.Key == "PRI" {
				detail += " PK"
			}
			items = append(items, CompletionItem{
				Label:  c.Name,
				Type:   "column",
				Detail: strings.TrimSpace(detail),
			})
		}
	}
	return items
}
// extractAliases builds a map of alias/tablename -> columns from FROM/JOIN clauses.
func extractAliases(sql string, db *SchemaForCompletion) map[string][]schema.Column {
	result := map[string][]schema.Column{}

	// Build table map
	tableMap := map[string][]schema.Column{}
	for _, s := range db.Schemas {
		for _, t := range s.Tables {
			tableMap[strings.ToLower(t.Name)] = t.Columns
		}
	}
	for _, t := range db.Tables {
		tableMap[strings.ToLower(t.Name)] = t.Columns
	}

	// Add all tables under their own name
	for name, cols := range tableMap {
		result[name] = cols
	}

	// Match FROM/JOIN aliases
	// We use a simple manual scan
	upper := strings.ToUpper(sql)
	words := strings.Fields(upper)
	for i := 0; i < len(words); i++ {
		if words[i] == "FROM" || words[i] == "JOIN" {
			if i+1 >= len(words) {
				continue
			}
			rawTable := words[i+1]
			// Handle schema.table
			if idx := strings.LastIndex(rawTable, "."); idx >= 0 {
				rawTable = rawTable[idx+1:]
			}
			rawTable = strings.Trim(rawTable, `"'` + "`" + `[]`)
			cols := tableMap[strings.ToLower(rawTable)]
			// Check for alias (next word after table, skipping AS)
			j := i + 2
			if j < len(words) && words[j] == "AS" {
				j++
			}
			if j < len(words) && !reservedWords[words[j]] {
				alias := strings.ToLower(strings.Trim(words[j], `"'` + "`" + `[]`))
				if cols != nil {
					result[alias] = cols
				}
			}
		}
	}

	return result
}
