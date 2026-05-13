import { html, nothing } from 'lit';
import { map } from 'lit/directives/map.js';
import { ColumnApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

const JUSTIFY: Record<string, string> = {
	start: 'justify-start',
	center: 'justify-center',
	end: 'justify-end',
	spaceBetween: 'justify-between',
	spaceAround: 'justify-around',
	spaceEvenly: 'justify-evenly',
	stretch: 'justify-stretch',
};

const ALIGN: Record<string, string> = {
	start: 'items-start',
	center: 'items-center',
	end: 'items-end',
	stretch: 'items-stretch',
};

export class NeboColumnElement extends A2uiLitElement<typeof ColumnApi> {
	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, ColumnApi);
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		const children = Array.isArray(props.children) ? props.children : [];
		const justify = JUSTIFY[props.justify || 'start'] || 'justify-start';
		const align = ALIGN[props.align || 'stretch'] || 'items-stretch';

		return html`
			<div class="flex flex-col gap-3 ${justify} ${align}">
				${map(children, (child: any) => this.renderNode(child))}
			</div>
		`;
	}
}

customElements.define('nebo-a2ui-column', NeboColumnElement);
export const NeboColumn = { ...ColumnApi, tagName: 'nebo-a2ui-column' };
