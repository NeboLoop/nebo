// A2UI type system — matches v1 views.json spec (a2ui.org v0.9)

/** A text value is either a literal string or a JSON pointer path for data binding */
export type TextValue = string | { path: string };

/** Children can be static ID refs or a list template that repeats per data item */
export type ChildRef = string[] | { componentId: string; path: string };

/** A single component definition in the flat adjacency list */
export interface A2UIComponentDef {
  id: string;
  component: string;
  props?: Record<string, unknown>;
  children?: ChildRef;
  action?: { name: string; payload?: Record<string, unknown> };
}

/** One view: a flat list of components + a data model + optional actions */
export interface A2UIView {
  components: A2UIComponentDef[];
  data: Record<string, unknown>;
  actions?: Record<string, { label: string; description?: string }>;
}

/** A navigation tab entry */
export interface A2UINavItem {
  viewId: string;
  label: string;
  icon?: string;
}

/** Full views config for an agent: nav + view map */
export interface A2UIViewsConfig {
  _nav: A2UINavItem[];
  [viewId: string]: A2UIView | A2UINavItem[];
}
