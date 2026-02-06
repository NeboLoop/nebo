---
name: database-expert
description: SQL query optimization, schema design, and database best practices
version: "1.0.0"
priority: 15
triggers:
  - sql
  - database
  - query
  - schema
  - table
  - index
  - migration
  - postgres
  - mysql
  - sqlite
  - optimize
tools:
  - read
  - write
  - edit
  - grep
  - glob
  - bash
metadata:
  nebo:
    emoji: "🗄️"
---

# Database Expert

When working with databases:

## 1. Query Optimization

- Use EXPLAIN/EXPLAIN ANALYZE to understand query plans
- Avoid `SELECT *` - select only needed columns
- Use appropriate indexes for WHERE, JOIN, ORDER BY
- Avoid N+1 queries - use JOINs or batch loading
- Use LIMIT for large result sets
- Consider query caching for repeated queries

## 2. Indexing Strategy

- Index columns used in WHERE clauses
- Index columns used in JOIN conditions
- Index columns used in ORDER BY
- Consider composite indexes for multi-column conditions
- Don't over-index - each index slows writes
- Use partial indexes when applicable

## 3. Schema Design

- Normalize to reduce redundancy (3NF for most cases)
- Denormalize for read-heavy workloads
- Use appropriate data types (smallest that fits)
- Add NOT NULL constraints where applicable
- Use foreign keys for referential integrity
- Consider soft deletes (`deleted_at`) vs hard deletes

## 4. Performance Tips

- Use connection pooling
- Batch inserts/updates when possible
- Use transactions appropriately
- Consider read replicas for scaling reads
- Use appropriate isolation levels
- Monitor slow query logs

## 5. Common Patterns

- **Pagination:** LIMIT/OFFSET or cursor-based
- **Soft deletes:** `deleted_at` timestamp
- **Audit trails:** `created_at`, `updated_at`, `created_by`
- **Versioning:** version column for optimistic locking

## Example: Query Optimization

**User:** "This query is slow, can you optimize it?"

**Original Query:**
```sql
SELECT * FROM orders o
WHERE o.user_id = 123
AND o.status = 'pending'
ORDER BY o.created_at DESC
```

**Issues Found:**
1. Using `SELECT *` - returns unnecessary columns
2. Missing index on frequently filtered columns

**Optimized Query:**
```sql
SELECT id, total, status, created_at
FROM orders
WHERE user_id = 123 AND status = 'pending'
ORDER BY created_at DESC
LIMIT 20
```

**Add Composite Index:**
```sql
CREATE INDEX idx_orders_user_status_created
ON orders (user_id, status, created_at DESC);
```

This index covers the WHERE clause and ORDER BY in one lookup.
