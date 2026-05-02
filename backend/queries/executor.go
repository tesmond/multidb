package queries

import (
	"context"
	"database/sql"
	"fmt"
	"strings"
	"time"
)

// QueryResult holds the result of a SQL query execution.
type QueryResult struct {
	Columns      []string `json:"columns"`
	ColumnTypes  []string `json:"columnTypes"` // database type names per column
	Rows         [][]any  `json:"rows"`
	RowsAffected int64    `json:"rowsAffected"`
	Duration     int64    `json:"duration"` // milliseconds
	Error        string   `json:"error,omitempty"`
}

// Executor runs SQL queries against a database connection.
type Executor struct{}

// NewExecutor creates a new Executor.
func NewExecutor() *Executor {
	return &Executor{}
}

// LooksLikeRowReturningQuery returns true when a statement is likely to return
// rows. Non-row statements should be executed via ExecContext so we can report
// affected row counts.
func LooksLikeRowReturningQuery(query string) bool {
	q := strings.TrimSpace(query)
	if q == "" {
		return false
	}
	up := strings.ToUpper(q)
	return strings.HasPrefix(up, "SELECT") ||
		strings.HasPrefix(up, "WITH") ||
		strings.HasPrefix(up, "SHOW") ||
		strings.HasPrefix(up, "DESCRIBE") ||
		strings.HasPrefix(up, "DESC ") ||
		strings.HasPrefix(up, "EXPLAIN") ||
		strings.HasPrefix(up, "PRAGMA") ||
		strings.HasPrefix(up, "VALUES")
}

// Execute runs a query and returns up to maxRows rows.
// Pass maxRows <= 0 to use the default limit of 1000.
func (e *Executor) Execute(ctx context.Context, db *sql.DB, query string, maxRows int) QueryResult {
	if maxRows <= 0 {
		maxRows = 1000000
	}

	if !LooksLikeRowReturningQuery(query) {
		return e.ExecuteNonQuery(ctx, db, query)
	}

	start := time.Now()

	rows, err := db.QueryContext(ctx, query)
	if err != nil {
		return QueryResult{
			Duration: time.Since(start).Milliseconds(),
			Error:    err.Error(),
		}
	}
	defer rows.Close()

	cols, err := rows.Columns()
	if err != nil {
		return QueryResult{
			Duration: time.Since(start).Milliseconds(),
			Error:    fmt.Sprintf("columns: %v", err),
		}
	}

	colTypeNames := make([]string, len(cols))
	if dbColTypes, err := rows.ColumnTypes(); err == nil {
		for i, ct := range dbColTypes {
			colTypeNames[i] = ct.DatabaseTypeName()
		}
	}

	result := QueryResult{
		Columns:     cols,
		ColumnTypes: colTypeNames,
		Rows:        make([][]any, 0),
	}

	for rows.Next() && len(result.Rows) < maxRows {
		// Check context cancellation
		select {
		case <-ctx.Done():
			result.Duration = time.Since(start).Milliseconds()
			result.Error = "query cancelled"
			return result
		default:
		}

		vals := make([]any, len(cols))
		ptrs := make([]any, len(cols))
		for i := range vals {
			ptrs[i] = &vals[i]
		}
		if err := rows.Scan(ptrs...); err != nil {
			result.Duration = time.Since(start).Milliseconds()
			result.Error = fmt.Sprintf("scan: %v", err)
			return result
		}

		// Convert []byte values to string for JSON serialization
		row := make([]any, len(cols))
		for i, v := range vals {
			switch val := v.(type) {
			case []byte:
				row[i] = string(val)
			default:
				row[i] = val
			}
		}
		result.Rows = append(result.Rows, row)
	}

	if err := rows.Err(); err != nil {
		result.Error = err.Error()
	}

	result.Duration = time.Since(start).Milliseconds()
	return result
}

// ExecuteNonQuery runs a non-SELECT statement (INSERT, UPDATE, DELETE, DDL).
func (e *Executor) ExecuteNonQuery(ctx context.Context, db *sql.DB, query string) QueryResult {
	start := time.Now()
	statements := splitStatements(query)
	var affected int64
	for _, stmt := range statements {
		res, err := db.ExecContext(ctx, stmt)
		if err != nil {
			return QueryResult{
				Duration: time.Since(start).Milliseconds(),
				Error:    err.Error(),
			}
		}
		if n, err := res.RowsAffected(); err == nil {
			affected += n
		}
	}

	return QueryResult{
		Duration:     time.Since(start).Milliseconds(),
		RowsAffected: affected,
		Columns:      []string{},
		Rows:         [][]any{},
	}
}

func splitStatements(query string) []string {
	raw := strings.Split(query, ";")
	out := make([]string, 0, len(raw))
	for _, s := range raw {
		t := strings.TrimSpace(s)
		if t == "" {
			continue
		}
		out = append(out, t)
	}
	if len(out) == 0 {
		return []string{strings.TrimSpace(query)}
	}
	return out
}
