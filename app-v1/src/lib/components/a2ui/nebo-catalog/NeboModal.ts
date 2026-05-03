import { html, nothing } from 'lit';
import { ModalApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

export class NeboModalElement extends A2uiLitElement<typeof ModalApi> {
	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, ModalApi);
	}

	private get _dialog(): HTMLDialogElement | null {
		return this.querySelector('dialog');
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		return html`
			<div @click=${() => this._dialog?.showModal()}>
				${props.trigger ? this.renderNode(props.trigger) : nothing}
			</div>
			<dialog class="modal modal-top sm:modal-middle">
				<div class="modal-box nebo-modal-box bg-base-200 shadow-xl rounded-xl max-h-[80vh] overflow-y-auto">
					<form method="dialog">
						<button class="btn btn-sm btn-circle btn-ghost absolute right-3 top-3">
							<span class="material-symbols-outlined text-base">close</span>
						</button>
					</form>
					${props.content ? this.renderNode(props.content) : nothing}
				</div>
				<form method="dialog" class="modal-backdrop">
					<button>close</button>
				</form>
			</dialog>
		`;
	}
}

customElements.define('nebo-a2ui-modal', NeboModalElement);
export const NeboModal = { ...ModalApi, tagName: 'nebo-a2ui-modal' };
