/**
 * Svelte action: set CSS custom properties on an element from script.
 *
 * STYLING EXCEPTION (documented, deliberate): some values are data, not
 * design, so they cannot live in app.css: the sheet viewer's per-cell
 * geometry and the workbook's own fill/ink colors (SheetView), and layout
 * measured at run time, such as the height of the composer floating over the
 * chat (`--chat-float`, ChatPane). They are handed to the DOM only as custom
 * properties set here, never as `style=` attributes or <style> blocks; every
 * rule that consumes them is static, a Tailwind utility in the markup
 * (`left-(--l)`, `bg-(--fill)`) or a rule in app.css. A number is in px.
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
