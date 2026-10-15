import { describe, expect, it } from 'vitest';
import { removeEmployee } from './removeEmployee';

const rows = [{ id: 'a' }, { id: 'b' }, { id: 'c' }];

describe('removeEmployee', () => {
  it('drops the row before the server answers', async () => {
    let shown = rows;
    let answer: () => void = () => {};
    const done = removeEmployee(rows, 'b', (r) => (shown = r), () => new Promise<void>((ok) => (answer = ok)));
    expect(shown.map((r) => r.id)).toEqual(['a', 'c']);
    answer();
    await done;
    expect(shown.map((r) => r.id)).toEqual(['a', 'c']);
  });

  it('puts the row back where it was when the server refuses', async () => {
    let shown = rows;
    await expect(
      removeEmployee(rows, 'b', (r) => (shown = r), () => Promise.reject(new Error('refused'))),
    ).rejects.toThrow('refused');
    expect(shown.map((r) => r.id)).toEqual(['a', 'b', 'c']);
  });
});
