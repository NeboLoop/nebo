import { html, nothing } from 'lit';
import { DividerApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

export class NeboDividerElement extends A2uiLitElement<typeof DividerApi> {
	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, DividerApi);
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		return props.axis === 'vertical'
			? html`<div class="divider divider-horizontal"></div>`
			: html`<div class="divider my-1"></div>`;
	}
}

customElements.define('nebo-a2ui-divider', NeboDividerElement);
export const NeboDivider = { ...DividerApi, tagName: 'nebo-a2ui-divider' };
