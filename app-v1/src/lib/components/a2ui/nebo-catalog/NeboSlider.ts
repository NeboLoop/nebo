import { html, nothing } from 'lit';
import { live } from 'lit/directives/live.js';
import { SliderApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

export class NeboSliderElement extends A2uiLitElement<typeof SliderApi> {
	// Local value tracks user interaction — needed because the A2UI binder's
	// setValue silently fails when the agent sends a literal value instead of
	// a data binding. We keep local state so the display always reflects the
	// user's chosen position.
	private _localValue: number | undefined = undefined;
	private _lastPropsValue: number | undefined = undefined;

	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, SliderApi);
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		// If props.value changed externally (agent pushed new data), clear local override
		if (props.value !== this._lastPropsValue) {
			this._lastPropsValue = props.value;
			this._localValue = undefined;
		}

		const displayValue = this._localValue ?? props.value ?? 0;

		return html`
			<div class="nebo-slider">
				${props.label
					? html`<label class="nebo-field-label">${props.label}</label>`
					: nothing}
				<input
					type="range"
					class="range range-primary w-full"
					min=${props.min ?? 0}
					max=${props.max ?? 100}
					step="any"
					.value=${live(displayValue.toString())}
					@input=${(e: Event) => {
						const val = Number((e.target as HTMLInputElement).value);
						this._localValue = val;
						props.setValue?.(val);
						this.requestUpdate();
					}}
				/>
				<span class="nebo-slider-value">${Number(displayValue.toFixed(1))}</span>
			</div>
		`;
	}
}

customElements.define('nebo-a2ui-slider', NeboSliderElement);
export const NeboSlider = { ...SliderApi, tagName: 'nebo-a2ui-slider' };
