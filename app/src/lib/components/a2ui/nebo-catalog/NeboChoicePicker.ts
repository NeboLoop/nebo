import { html, nothing } from 'lit';
import { ChoicePickerApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

export class NeboChoicePickerElement extends A2uiLitElement<typeof ChoicePickerApi> {
	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, ChoicePickerApi);
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		const selected: string[] = Array.isArray(props.value) ? props.value : [];
		const isMulti = props.variant === 'multipleSelection';

		const toggle = (val: string) => {
			if (!props.setValue) return;
			if (isMulti) {
				if (selected.includes(val)) {
					props.setValue(selected.filter((v: string) => v !== val));
				} else {
					props.setValue([...selected, val]);
				}
			} else {
				props.setValue([val]);
			}
		};

		const inputType = isMulti ? 'checkbox' : 'radio';
		const inputClass = isMulti
			? 'checkbox checkbox-primary'
			: 'radio radio-primary';

		return html`
			<div class="nebo-choicepicker">
				${props.label ? html`<label class="nebo-field-label">${props.label}</label>` : nothing}
				<div class="nebo-option-group">
					${props.options?.map(
						(opt: any) => html`
							<label class="nebo-option-label">
								<input
									type=${inputType}
									class=${inputClass}
									.checked=${selected.includes(opt.value)}
									@change=${() => toggle(opt.value)}
								/>
								<span>${opt.label}</span>
							</label>
						`
					)}
				</div>
			</div>
		`;
	}
}

customElements.define('nebo-a2ui-choicepicker', NeboChoicePickerElement);
export const NeboChoicePicker = { ...ChoicePickerApi, tagName: 'nebo-a2ui-choicepicker' };
