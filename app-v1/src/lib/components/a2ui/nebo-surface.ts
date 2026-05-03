/**
 * NeboSurfaceElement — custom A2UI surface that injects document styles
 * into the shadow DOM.
 *
 * The upstream <a2ui-surface> uses shadow DOM, which blocks global CSS
 * (Tailwind, DaisyUI, our A2UI theme) from reaching the component tree.
 * This subclass keeps shadow DOM for agent CSS isolation, but clones
 * document <style> and <link> elements into the shadow root so our
 * design system styles apply.
 *
 * A MutationObserver watches document.head for dynamically-added
 * stylesheets (Vite HMR in dev, agent themes via loadAgentTheme()).
 */
import { A2uiSurface } from '@a2ui/lit/v0_9';
import { ContextProvider } from '@lit/context';
import { neboActionContext, type NeboActionState, type ActionCompleteListener } from './nebo-action-context';

export class NeboSurfaceElement extends A2uiSurface {
	private _styleObserver?: MutationObserver;
	private _stylesInjected = false;
	private _completionListeners = new Set<ActionCompleteListener>();

	private _actionProvider = new ContextProvider(this, {
		context: neboActionContext,
		initialValue: {
			onComplete: (cb: ActionCompleteListener) => {
				this._completionListeners.add(cb);
				return () => this._completionListeners.delete(cb);
			},
		},
	});

	/** Called from A2UISurfacePanel when an action completes.
	 *  Notifies all registered buttons to clear their loading state. */
	notifyActionComplete() {
		this._completionListeners.forEach((cb) => cb());
	}

	// Use firstUpdated (not connectedCallback) because LitElement creates
	// the shadow root lazily during the first update cycle.
	firstUpdated(changedProperties: Map<string, unknown>) {
		super.firstUpdated?.(changedProperties);
		this._injectDocumentStyles();
	}

	disconnectedCallback() {
		super.disconnectedCallback();
		this._styleObserver?.disconnect();
		this._styleObserver = undefined;
	}

	/** Clone a stylesheet element for shadow root injection, enabling agent themes. */
	private _cloneForShadow(el: Element): Element {
		const clone = el.cloneNode(true) as Element;
		clone.setAttribute('data-nebo-injected', '');
		// Agent theme styles use media="not all" in document.head to prevent
		// global leakage. Enable them inside the shadow root.
		if (clone instanceof HTMLStyleElement && clone.dataset.a2uiTheme) {
			clone.media = '';
		}
		return clone;
	}

	private _injectDocumentStyles() {
		const root = this.shadowRoot;
		if (!root || this._stylesInjected) return;
		this._stylesInjected = true;

		// Clone all existing document stylesheets into shadow root
		const nodes = document.head.querySelectorAll('style, link[rel="stylesheet"]');
		Array.from(nodes).forEach((el) => {
			root.prepend(this._cloneForShadow(el));
		});

		// Watch for new stylesheets (Vite HMR + agent themes loaded dynamically)
		this._styleObserver = new MutationObserver((mutations) => {
			for (const m of mutations) {
				m.addedNodes.forEach((node) => {
					if (
						node instanceof HTMLStyleElement ||
						(node instanceof HTMLLinkElement && node.rel === 'stylesheet')
					) {
						root.prepend(this._cloneForShadow(node as Element));
					}
				});
			}
		});
		this._styleObserver.observe(document.head, { childList: true });
	}
}

if (!customElements.get('nebo-a2ui-surface')) {
	customElements.define('nebo-a2ui-surface', NeboSurfaceElement);
}

declare global {
	interface HTMLElementTagNameMap {
		'nebo-a2ui-surface': NeboSurfaceElement;
	}
}
