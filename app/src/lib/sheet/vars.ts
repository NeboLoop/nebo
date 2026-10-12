/**
 * Svelte action: set CSS custom properties on an element from script.
 *
 * STYLING EXCEPTION (documented, deliberate): a faithful grid needs each
 * cell's position and size and the file's own fill/ink colours. Those values
 * are data from the workbook, not design, so they cannot live in app.css.
 * They are handed to the DOM only as custom properties (`--l`, `--fill`, …)
 * set here, never as `style=` attributes or <style> blocks; every rule that
 * consumes them is a static Tailwind utility in the markup (`left-(--l)`,
 * `bg-(--fill)`). Nothing else in the sheet viewer sets styles from script.
 */
export type CssVars = Record<`--${string}`, string | number | undefined | null>;

export function vars(node: HTMLElement, initial: CssVars) {
	let prev: CssVars = {};
	const apply = (next: CssVars) => {
		for (const k of Object.keys(prev) as (keyof CssVars)[]) {
			if (!(k in next) || next[k] == null) node.style.removeProperty(k);
		}
		for (const [k, v] of Object.entries(next)) {
			if (v == null) continue;
			const s = typeof v === 'number' ? `${v}px` : v;
			if (node.style.getPropertyValue(k) !== s) node.style.setProperty(k, s);
		}
		prev = next;
	};
	apply(initial);
	return { update: apply };
}
