---
name: api-design
description: RESTful API design best practices and endpoint implementation
version: "1.0.0"
priority: 15
triggers:
  - api
  - endpoint
  - rest
  - restful
  - http
  - route
  - handler
tools:
  - read
  - write
  - edit
  - grep
  - glob
metadata:
  nebo:
    emoji: "🔌"
---

# API Design

When designing or reviewing APIs:

## 1. RESTful Design Principles

- Use nouns for resources (users, orders, products)
- Use HTTP verbs correctly:
  - **GET**: Read (safe, idempotent)
  - **POST**: Create
  - **PUT**: Replace (idempotent)
  - **PATCH**: Partial update
  - **DELETE**: Remove (idempotent)
- Use plural nouns for collections (`/users` not `/user`)
- Use nested resources for relationships (`/users/{id}/orders`)

## 2. URL Structure

```
/api/v1/resources          - versioned API
/resources/{id}            - specific resource
/resources/{id}/subresources - nested resources
```

Query parameters:
- Filtering: `/users?status=active`
- Pagination: `?page=1&limit=20`

## 3. HTTP Status Codes

| Code | Meaning | When to Use |
|------|---------|-------------|
| 200 | OK | Successful GET, PUT, PATCH |
| 201 | Created | Successful POST |
| 204 | No Content | Successful DELETE |
| 400 | Bad Request | Invalid input |
| 401 | Unauthorized | Authentication required |
| 403 | Forbidden | Authenticated but not authorized |
| 404 | Not Found | Resource doesn't exist |
| 409 | Conflict | Resource state conflict |
| 422 | Unprocessable Entity | Validation errors |
| 429 | Too Many Requests | Rate limited |
| 500 | Internal Server Error | Server-side error |

## 4. Response Format

- Consistent JSON structure
- Include metadata for collections (total, page, limit)
- Use camelCase for JSON keys
- Return created/updated resource in response

Error format:
```json
{
  "error": {
    "code": "VALIDATION_ERROR",
    "message": "Description",
    "details": [...]
  }
}
```

## 5. Security

- Always use HTTPS
- Use JWT or API keys for authentication
- Validate all inputs
- Rate limit endpoints
- Add CORS headers appropriately

## Example

**User:** "Design an API for managing products"

**Assistant:**

### Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | /api/v1/products | List products (with pagination) |
| GET | /api/v1/products/{id} | Get product by ID |
| POST | /api/v1/products | Create product |
| PUT | /api/v1/products/{id} | Replace product |
| PATCH | /api/v1/products/{id} | Update product fields |
| DELETE | /api/v1/products/{id} | Delete product |

### Response Example

**GET /api/v1/products**
```json
{
  "data": [
    {"id": "1", "name": "Widget", "price": 29.99}
  ],
  "meta": {
    "total": 100,
    "page": 1,
    "limit": 20
  }
}
```
