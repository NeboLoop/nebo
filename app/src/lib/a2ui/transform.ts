// Transform backend views.json format → client A2Node format
//
// Backend (views.json):      PascalCase components, flat props, child/children singular, action.event
// Client (A2UIView):         lowercase components, props object, children array, action.name/payload

import type { A2UIView, A2UIComponentDef, A2UINavItem, A2UIViewsConfig } from './types.js';

/** Known props that should be moved into the props bag */
const PROP_KEYS = new Set([
  'text', 'variant', 'justify', 'align', 'gap', 'weight', 'fit', 'url',
  'label', 'size', 'name', 'wrap', 'class', 'description', 'value',
  'change', 'content', 'placeholder', 'level', 'direction', 'surface_type',
  'tabs',
]);

/**
 * Transform a single backend component into an A2UIComponentDef.
 */
function transformComponent(raw: Record<string, unknown>): A2UIComponentDef {
  const id = raw.id as string;
  let component = (raw.component as string || '').toLowerCase();

  // List → column (lists are just columns with template children)
  if (component === 'list') component = 'column';

  // Tabs: convert { tabs: [{ title, child }] } → props.tabs + children
  if (component === 'tabs' && Array.isArray(raw.tabs)) {
    const tabDefs = raw.tabs as { title: string; child: string }[];
    const props: Record<string, unknown> = {
      tabs: tabDefs.map(t => ({ id: t.child, label: t.title })),
    };
    return {
      id,
      component: 'tabs',
      props,
      children: tabDefs.map(t => t.child),
    };
  }

  // Build props from flat keys
  const props: Record<string, unknown> = {};
  for (const [key, val] of Object.entries(raw)) {
    if (key === 'id' || key === 'component' || key === 'children' || key === 'child' || key === 'action') continue;
    if (PROP_KEYS.has(key)) {
      props[key] = val;
    }
  }

  // Merge any existing props object
  if (raw.props && typeof raw.props === 'object') {
    Object.assign(props, raw.props);
  }

  const def: A2UIComponentDef = { id, component };

  if (Object.keys(props).length > 0) {
    def.props = props;
  }

  // Normalize children: "child": "id" → children: ["id"]
  if (raw.children !== undefined) {
    if (Array.isArray(raw.children)) {
      // Already an array of IDs or a template ref
      def.children = raw.children;
    } else if (typeof raw.children === 'object' && raw.children !== null) {
      // Template ref: { componentId, path } — pass through
      def.children = raw.children as { componentId: string; path: string };
    } else if (typeof raw.children === 'string') {
      def.children = [raw.children];
    }
  } else if (raw.child !== undefined) {
    if (typeof raw.child === 'string') {
      def.children = [raw.child];
    }
  }

  // Transform action: { event: { name, context } } → { name, payload }
  if (raw.action) {
    const action = raw.action as Record<string, unknown>;
    if (action.event && typeof action.event === 'object') {
      const event = action.event as Record<string, unknown>;
      def.action = {
        name: event.name as string,
        payload: (event.context as Record<string, unknown>) ?? undefined,
      };
    } else if (action.name) {
      // Already in client format
      def.action = {
        name: action.name as string,
        payload: (action.payload as Record<string, unknown>) ?? undefined,
      };
    }
  }

  return def;
}

/**
 * Transform a single backend view definition into an A2UIView.
 */
export function transformView(backendView: Record<string, unknown>): A2UIView {
  const rawComponents = (backendView.components as Record<string, unknown>[]) || [];
  const components = rawComponents.map(transformComponent);
  const data = (backendView.data as Record<string, unknown>) || {};
  const actions = backendView.actions as Record<string, { label: string; description?: string }> | undefined;

  return { components, data, actions };
}

/**
 * Transform an entire backend views.json object into A2UIViewsConfig.
 * Handles _nav and all view entries.
 */
export function transformViewsConfig(backendViews: Record<string, unknown>): A2UIViewsConfig {
  // Extract or auto-generate nav
  let nav: A2UINavItem[];
  if (Array.isArray(backendViews._nav)) {
    nav = backendViews._nav as A2UINavItem[];
  } else {
    // Auto-generate nav from view keys
    nav = Object.keys(backendViews)
      .filter(k => !k.startsWith('_'))
      .map(k => ({
        viewId: k,
        label: k === 'default' ? 'Dashboard' : k.replace(/-/g, ' ').replace(/\b\w/g, c => c.toUpperCase()),
      }));
  }

  const config: A2UIViewsConfig = { _nav: nav };

  for (const [key, value] of Object.entries(backendViews)) {
    if (key === '_nav') continue;
    if (typeof value === 'object' && value !== null && !Array.isArray(value)) {
      config[key] = transformView(value as Record<string, unknown>);
    }
  }

  return config;
}
