import { html, nothing } from 'lit';
import { TextFieldApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

export class NeboTextFieldElement extends A2uiLitElement<typeof TextFieldApi> {
	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, TextFieldApi);
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		const isInvalid = props.isValid === false;
		const onInput = (e: Event) => props.setValue?.((e.target as HTMLInputElement).value);

		let type = 'text';
		if (props.variant === 'number') type = 'number';
		if (props.variant === 'obscured') type = 'password';

		return html`
			<div class="nebo-textfield">
				${props.label ? html`<label class="nebo-field-label">${props.label}</label>` : nothing}
				${props.variant === 'longText'
					? html`<textarea
							class="textarea textarea-bordered w-full ${isInvalid ? 'textarea-error' : ''}"
							.value=${props.value || ''}
							@input=${onInput}
						></textarea>`
					: html`<input
							type=${type}
							class="input input-bordered w-full ${isInvalid ? 'input-error' : ''}"
							.value=${props.value || ''}
							@input=${onInput}
						/>`}
				${isInvalid && props.validationErrors?.length
					? html`<div class="text-error text-xs mt-1">${props.validationErrors[0]}</div>`
					: nothing}
			</div>
		`;
	}
}

customElements.define('nebo-a2ui-textfield', NeboTextFieldElement);
export const NeboTextField = { ...TextFieldApi, tagName: 'nebo-a2ui-textfield' };
