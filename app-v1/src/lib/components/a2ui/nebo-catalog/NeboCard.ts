import { html, nothing } from 'lit';
import { CardApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

export class NeboCardElement extends A2uiLitElement<typeof CardApi> {
	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, CardApi);
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		return html`
			<div class="card nebo-card bg-base-200 rounded-xl p-5">
				${props.child ? this.renderNode(props.child) : nothing}
			</div>
		`;
	}
}

customElements.define('nebo-a2ui-card', NeboCardElement);
export const NeboCard = { ...CardApi, tagName: 'nebo-a2ui-card' };
