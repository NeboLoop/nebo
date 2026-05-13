import { html, nothing } from 'lit';
import { CheckBoxApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

export class NeboCheckBoxElement extends A2uiLitElement<typeof CheckBoxApi> {
	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, CheckBoxApi);
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		return html`
			<label class="nebo-option-label">
				<input
					type="checkbox"
					class="checkbox checkbox-primary"
					.checked=${props.value || false}
					@change=${(e: Event) => props.setValue?.((e.target as HTMLInputElement).checked)}
				/>
				<span>${props.label}</span>
			</label>
		`;
	}
}

customElements.define('nebo-a2ui-checkbox', NeboCheckBoxElement);
export const NeboCheckBox = { ...CheckBoxApi, tagName: 'nebo-a2ui-checkbox' };
