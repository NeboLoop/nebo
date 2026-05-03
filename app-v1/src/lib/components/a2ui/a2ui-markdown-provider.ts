/**
 * Custom element that provides a markdown renderer via the @lit/context
 * protocol (context-request DOM events).
 *
 * Wrap <a2ui-surface> inside <a2ui-markdown-provider> so that
 * Text components can render heading variants (h1-h5) as actual
 * HTML headings instead of literal "#" markers.
 *
 * Uses STATIC imports so the custom element is defined synchronously —
 * this prevents a race where Text components fire context-request before
 * the provider is registered.
 */
import { Context } from '@a2ui/lit/v0_9';
import { Marked } from 'marked';

const markedInstance = new Marked({ gfm: true, breaks: false });

async function renderMarkdown(text: string): Promise<string> {
	return await markedInstance.parse(text);
}

if (typeof window !== 'undefined') {
	class A2UIMarkdownProvider extends HTMLElement {
		connectedCallback() {
			this.addEventListener('context-request', this._onContextRequest as EventListener);
		}

		disconnectedCallback() {
			this.removeEventListener('context-request', this._onContextRequest as EventListener);
		}

		private _onContextRequest = (ev: Event) => {
			const req = ev as Event & { context: unknown; callback: (value: unknown) => void };
			if (req.context !== Context.markdown) return;
			ev.stopPropagation();
			if (typeof req.callback === 'function') {
				req.callback(renderMarkdown);
			}
		};
	}

	if (!customElements.get('a2ui-markdown-provider')) {
		customElements.define('a2ui-markdown-provider', A2UIMarkdownProvider);
	}
}

declare global {
	interface HTMLElementTagNameMap {
		'a2ui-markdown-provider': HTMLElement;
	}
}
