package queries

import (
	"context"
	"database/sql"
	"fmt"
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

// Execute runs a query and returns up to maxRows rows.
// Pass maxRows <= 0 to use the default limit of 1000.
func (e *Executor) Execute(ctx context.Context, db *sql.DB, query string, maxRows int) QueryResult {
	if maxRows <= 0 {
		maxRows = 1000000
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

	res, err := db.ExecContext(ctx, query)
	if err != nil {
		return QueryResult{
			Duration: time.Since(start).Milliseconds(),
			Error:    err.Error(),
		}
	}

	affected, _ := res.RowsAffected()
	return QueryResult{
		Duration:     time.Since(start).Milliseconds(),
		RowsAffected: affected,
		Columns:      []string{},
		Rows:         [][]any{},
	}
}
