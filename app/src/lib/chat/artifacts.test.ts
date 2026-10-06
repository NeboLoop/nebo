import { describe, it, expect } from 'vitest';
import { artifactsToAttachments, artifactsToWorkItems } from './controller.svelte';

describe('run artifacts in the chat', () => {
  it('shows audio as an inline player, never a Document card', () => {
    const artifacts = ['/api/v1/files/.shared/0123456789abcdef/song.mp3', '/api/v1/files/voice.M4A'];
    const atts = artifactsToAttachments(artifacts);
    expect(atts.map((a) => [a.filename, a.mimeType])).toEqual([
      ['song.mp3', 'audio/mpeg'],
      ['voice.M4A', 'audio/mp4'],
    ]);
    expect(artifactsToWorkItems(artifacts)).toEqual([]);
  });

  it('plays an audio file an older message kept as a versioned document', () => {
    const kept = { documentId: 'd1', filename: 'song.mp3', kind: 'document', version: 1, url: '/api/v1/files/work/blobs/abc.mp3' };
    expect(artifactsToAttachments([kept])).toEqual([
      { fileId: '', filename: 'song.mp3', mimeType: 'audio/mpeg', size: 0, url: kept.url },
    ]);
    expect(artifactsToWorkItems([kept])).toEqual([]);
  });

  it('names a shared file by its own name, not its folder', () => {
    const [clip] = artifactsToAttachments(['/api/v1/files/.shared/0123456789abcdef/clip.mp4']);
    expect(clip.filename).toBe('clip.mp4');
    expect(clip.mimeType).toBe('video/mp4');
  });

  it('keeps documents in the Work panel', () => {
    const doc = { documentId: 'd2', filename: 'report.md', kind: 'document', version: 2, url: '/api/v1/files/work/blobs/def.md' };
    expect(artifactsToAttachments([doc])).toEqual([]);
    expect(artifactsToWorkItems([doc]).map((w) => w.title)).toEqual(['report.md']);
  });

  it('shows one card per document at its latest version, in first-seen order', () => {
    const v = (id: string, filename: string, version: number) =>
      ({ documentId: id, filename, kind: 'document', version, url: `/api/v1/files/work/blobs/${id}${version}.md` });
    // One reply wrote nebo-x-article.md three times (2026-10-06).
    const one = artifactsToWorkItems([v('f16e', 'nebo-x-article.md', 2), v('f16e', 'nebo-x-article.md', 3), v('f16e', 'nebo-x-article.md', 4)]);
    expect(one.map((w) => [w.title, w.version])).toEqual([['nebo-x-article.md', 4]]);
    // Two files: two cards, each at its newest version.
    const two = artifactsToWorkItems([v('a', 'a.md', 1), v('b', 'b.md', 1), v('a', 'a.md', 2)]);
    expect(two.map((w) => [w.title, w.version])).toEqual([['a.md', 2], ['b.md', 1]]);
    // Bare URLs of one file collapse to the last write.
    const bare = artifactsToWorkItems(['/api/v1/files/.shared/1111/notes.md', '/api/v1/files/.shared/2222/notes.md']);
    expect(bare.map((w) => w.url)).toEqual(['/api/v1/files/.shared/2222/notes.md']);
  });
});
