import { html, nothing } from 'lit';
import { ButtonApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';
import { ContextConsumer } from '@lit/context';
import { neboActionContext } from '../nebo-action-context';

const VARIANT_CLASS: Record<string, string> = {
	default: 'btn-soft',
	primary: 'btn-primary',
	secondary: 'btn-secondary',
	outline: 'btn-outline',
	borderless: 'btn-ghost',
};

export class NeboButtonElement extends A2uiLitElement<typeof ButtonApi> {
	private _pending = false;
	private _unsubComplete?: () => void;

	// Consume action state from the ancestor NeboSurfaceElement via Lit context
	private _actionConsumer = new ContextConsumer(this, {
		context: neboActionContext,
		callback: (actionState) => {
			// Re-register completion listener when context changes
			this._unsubComplete?.();
			this._unsubComplete = actionState?.onComplete(() => {
				if (this._pending) {
					this._pending = false;
					this.requestUpdate();
				}
			});
		},
	});

	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, ButtonApi);
	}

	disconnectedCallback() {
		super.disconnectedCallback();
		this._unsubComplete?.();
		this._unsubComplete = undefined;
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		const disabled = props.isValid === false || this._pending;
		const variant = VARIANT_CLASS[props.variant || 'default'] || '';

		return html`
			<button
				class="btn ${variant}"
				@click=${() => {
					if (disabled) return;
					this._pending = true;
					this.requestUpdate();
					props.action?.();
				}}
				?disabled=${disabled}
			>
				${this._pending
					? html`<span class="loading loading-spinner loading-xs"></span>`
					: props.child ? this.renderNode(props.child) : nothing}
			</button>
		`;
	}
}

customElements.define('nebo-a2ui-button', NeboButtonElement);
export const NeboButton = { ...ButtonApi, tagName: 'nebo-a2ui-button' };
