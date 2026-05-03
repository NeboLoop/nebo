import { html, nothing } from 'lit';
import { IconApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

export class NeboIconElement extends A2uiLitElement<typeof IconApi> {
	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, IconApi);
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		const name = typeof props.name === 'string' ? props.name : (props.name as any)?.path;

		return html`<span class="material-symbols-outlined nebo-icon">${name}</span>`;
	}
}

customElements.define('nebo-a2ui-icon', NeboIconElement);
export const NeboIcon = { ...IconApi, tagName: 'nebo-a2ui-icon' };
