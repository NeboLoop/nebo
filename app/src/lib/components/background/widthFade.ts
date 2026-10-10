import { cubicOut } from 'svelte/easing';

/** A piece of a one-line strip coming or going: it opens to its width and
 *  fades in (or closes and fades out), so the pieces beside it slide rather
 *  than jump. Instant when the owner asks for reduced motion. */
export function widthFade(node: HTMLElement, { duration = 180 }: { duration?: number } = {}) {
	const reduced = typeof window !== 'undefined' && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
	const width = node.offsetWidth;
	return {
		duration: reduced ? 0 : duration,
		easing: cubicOut,
		css: (t: number) => `overflow: hidden; white-space: nowrap; width: ${t * width}px; opacity: ${t};`,
	};
}
