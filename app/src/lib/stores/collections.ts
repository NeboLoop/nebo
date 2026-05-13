import { writable, derived } from 'svelte/store';

export interface PrivateCollection {
  id: string;
  name: string;
  desc: string;
  orgId: string;
  items: string[];
  itemCount: number;
  curator: string;
  updated: string;
  visibility: 'public' | 'private';
}

export const collections = writable<PrivateCollection[]>([]);

let nextId = 1;

export function createCollection(col: Omit<PrivateCollection, 'id' | 'updated'>) {
  const id = `col-${nextId++}`;
  collections.update(list => [
    ...list,
    { ...col, id, updated: 'Just now' },
  ]);
  return id;
}

export function deleteCollection(id: string) {
  collections.update(list => list.filter(c => c.id !== id));
}

export function addItemToCollection(collectionId: string, itemId: string) {
  collections.update(list => list.map(c => {
    if (c.id !== collectionId) return c;
    if (c.items.includes(itemId)) return c;
    const items = [...c.items, itemId];
    return { ...c, items, itemCount: items.length, updated: 'Just now' };
  }));
}

export function removeItemFromCollection(collectionId: string, itemId: string) {
  collections.update(list => list.map(c => {
    if (c.id !== collectionId) return c;
    const items = c.items.filter(id => id !== itemId);
    return { ...c, items, itemCount: items.length, updated: 'Just now' };
  }));
}

export function collectionsForOrg(orgId: string) {
  return derived(collections, ($cols) => $cols.filter(c => c.orgId === orgId));
}
