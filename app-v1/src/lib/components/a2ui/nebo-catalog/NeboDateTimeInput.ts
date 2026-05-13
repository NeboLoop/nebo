import { html, nothing } from 'lit';
import { DateTimeInputApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

export class NeboDateTimeInputElement extends A2uiLitElement<typeof DateTimeInputApi> {
	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, DateTimeInputApi);
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		const type =
			props.enableDate && props.enableTime
				? 'datetime-local'
				: props.enableDate
					? 'date'
					: 'time';

		return html`
			<div class="nebo-textfield">
				${props.label ? html`<label class="nebo-field-label">${props.label}</label>` : nothing}
				<input
					type=${type}
					class="input input-sm input-bordered w-full"
					.value=${props.value || ''}
					min=${props.min || ''}
					max=${props.max || ''}
					@input=${(e: Event) => props.setValue?.((e.target as HTMLInputElement).value)}
				/>
			</div>
		`;
	}
}

customElements.define('nebo-a2ui-datetimeinput', NeboDateTimeInputElement);
export const NeboDateTimeInput = { ...DateTimeInputApi, tagName: 'nebo-a2ui-datetimeinput' };
