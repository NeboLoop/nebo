/**
 * Lit context for action pending state.
 *
 * NeboSurfaceElement provides this context. NeboButtonElement consumes it.
 * When an action completes, the surface notifies all registered buttons
 * so they can clear their loading spinners.
 */
import { createContext } from '@lit/context';

export type ActionCompleteListener = () => void;

export interface NeboActionState {
	/** Register a callback invoked when any action on this surface completes.
	 *  Returns an unsubscribe function. */
	onComplete: (cb: ActionCompleteListener) => () => void;
}

export const neboActionContext = createContext<NeboActionState>('nebo-action-state');
