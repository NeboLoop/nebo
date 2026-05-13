// Data binding resolver for A2UI views
// Resolves JSON pointer paths against root data and optional scope data

import type { TextValue } from './types.js';

/**
 * Resolve a single value from a TextValue.
 * - string literal → returned as-is
 * - { path: "/foo/bar" } → resolved against rootData (absolute)
 * - { path: "field" } → resolved against scopeData first, then rootData (relative)
 */
export function resolveValue(
  tv: TextValue | undefined,
  rootData: Record<string, unknown>,
  scopeData?: Record<string, unknown>
): string {
  if (tv === undefined || tv === null) return '';
  if (typeof tv === 'string') return tv;

  const path = tv.path;
  if (!path) return '';

  // Absolute path starts with /
  if (path.startsWith('/')) {
    return String(getByPointer(rootData, path) ?? '');
  }

  // Relative path — check scope first, then root
  if (scopeData && path in scopeData) {
    return String(scopeData[path] ?? '');
  }
  return String((rootData as Record<string, unknown>)[path] ?? '');
}

/**
 * Resolve an array from a JSON pointer path for list iteration.
 */
export function resolveArray(
  path: string,
  rootData: Record<string, unknown>,
  scopeData?: Record<string, unknown>
): unknown[] {
  let result: unknown;
  if (path.startsWith('/')) {
    result = getByPointer(rootData, path);
  } else if (scopeData && path in scopeData) {
    result = scopeData[path];
  } else {
    result = (rootData as Record<string, unknown>)[path];
  }
  return Array.isArray(result) ? result : [];
}

/**
 * Navigate an object via JSON pointer (RFC 6901).
 * "/metrics/total" → obj.metrics.total
 */
function getByPointer(obj: unknown, pointer: string): unknown {
  const parts = pointer.split('/').filter(Boolean);
  let current: unknown = obj;
  for (const part of parts) {
    if (current == null || typeof current !== 'object') return undefined;
    current = (current as Record<string, unknown>)[part];
  }
  return current;
}
