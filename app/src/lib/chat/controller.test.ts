import { describe, expect, it } from 'vitest';
import type { WorkDocumentListing } from '$lib/api/neboComponents';
import { artifactsToAttachments, artifactsToWorkItems, kindForExt, mergeDocumentVersions, workDocumentsToItems } from './controller.svelte';

describe('kindForExt', () => {
  it('files audio as audio', () => {
    for (const ext of ['wav', 'mp3', 'm4a', 'aac', 'ogg', 'opus']) expect(kindForExt(ext)).toBe('audio');
  });

  it('keeps the other kinds', () => {
    expect(kindForExt('csv')).toBe('table');
    expect(kindForExt('pptx')).toBe('slides');
    expect(kindForExt('py')).toBe('code');
    expect(kindForExt('pdf')).toBe('document');
  });
});

// A shared .wav is a Work item that plays, never inline media — and rows
// written before the `audio` kind existed call it a document.
describe('artifactsToWorkItems audio', () => {
  const wav = { documentId: 'd1', filename: 'take.wav', kind: 'document', version: 1, url: '/api/v1/files/c/take.wav' };

  it('decides audio by extension over an older row’s document kind', () => {
    expect(artifactsToWorkItems([wav])[0].kind).toBe('audio');
  });

  it('takes the audio kind from a new row', () => {
    expect(artifactsToWorkItems([{ ...wav, kind: 'audio' }])[0].kind).toBe('audio');
  });

  it('files a legacy bare audio URL as audio work, not inline media', () => {
    const url = '/api/v1/files/c/voice.mp3';
    expect(artifactsToWorkItems([url])[0]).toMatchObject({ kind: 'audio', title: 'voice.mp3', version: 1 });
    expect(artifactsToAttachments([url])).toEqual([]);
  });

  it('keeps webm as inline video', () => {
    const url = '/api/v1/files/c/clip.webm';
    expect(artifactsToWorkItems([url])).toEqual([]);
    expect(artifactsToAttachments([url])[0].mimeType).toBe('video/webm');
  });
});

describe('workDocumentsToItems', () => {
  const row = (over: Partial<WorkDocumentListing>): WorkDocumentListing => ({
    id: 'd1', chatId: 'chat-1', filename: 'report.md', kind: 'document', latestVersion: 1,
    url: '/api/v1/files/chat-1/report.md', createdAt: 0, updatedAt: 0, ...over,
  });

  it('maps a listing to its latest version as a Work item', () => {
    const items = workDocumentsToItems([row({ id: 'd2', filename: 'take.wav', kind: 'document', latestVersion: 3, url: '/api/v1/files/v3/take.wav' })], 'chat-1');
    expect(items).toEqual([
      { id: 'd2', documentId: 'd2', title: 'take.wav', kind: 'audio', version: 3, url: '/api/v1/files/v3/take.wav', codeUrl: undefined },
    ]);
  });

  it('drops another chat’s documents (an older server ignores chatId)', () => {
    const items = workDocumentsToItems([row({ id: 'mine' }), row({ id: 'theirs', chatId: 'chat-2' })], 'chat-1');
    expect(items.map((i) => i.id)).toEqual(['mine']);
  });

  it('pairs a compiled html with its source like the live cards', () => {
    const items = workDocumentsToItems([
      row({ id: 'h', filename: 'app.html', url: '/api/v1/files/c/app.html' }),
      row({ id: 'j', filename: 'app.jsx', kind: 'code', url: '/api/v1/files/c/app.jsx' }),
    ], 'chat-1');
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ id: 'h', codeUrl: '/api/v1/files/c/app.jsx' });
  });

  it('tolerates a missing list', () => {
    expect(workDocumentsToItems(undefined, 'chat-1')).toEqual([]);
  });
});

describe('mergeDocumentVersions', () => {
  type V = { documentId: string; version: number; from: string };
  const v = (documentId: string, version: number, from: string): V => ({ documentId, version, from });

  it('keeps a document only the server knows (made before the loaded page)', () => {
    const map = mergeDocumentVersions<V>([], [v('old', 2, 'server')]);
    expect(map.get('old')).toEqual([v('old', 2, 'server')]);
  });

  it('dedupes a version both know, keeping the message copy', () => {
    const map = mergeDocumentVersions<V>([v('d', 2, 'message')], [v('d', 2, 'server')]);
    expect(map.get('d')).toEqual([v('d', 2, 'message')]);
  });

  it('adds a newer live version after the server’s latest, oldest first', () => {
    const map = mergeDocumentVersions<V>([v('d', 1, 'message'), v('d', 3, 'message')], [v('d', 2, 'server')]);
    expect(map.get('d')?.map((x) => x.version)).toEqual([1, 2, 3]);
  });

  it('orders documents oldest first, then those only messages know', () => {
    // The server lists newest first.
    const map = mergeDocumentVersions<V>([v('live', 1, 'message'), v('b', 1, 'message')], [v('b', 1, 'server'), v('a', 1, 'server')]);
    expect([...map.keys()]).toEqual(['a', 'b', 'live']);
  });
});
