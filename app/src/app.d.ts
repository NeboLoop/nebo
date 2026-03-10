// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
declare global {
	namespace App {
		// interface Error {}
		// interface Locals {}
		// interface PageData {}
		// interface PageState {}
		// interface Platform {}
	}
}

// Vite environment variable types (build-time)
interface ImportMetaEnv {
	readonly DEV: boolean;
	readonly PROD: boolean;
	readonly MODE: string;
	readonly VITE_ENVIRONMENT?: string;
	readonly VITE_APP_VERSION?: string;
	readonly VITE_ENABLE_DEBUG?: string;
	readonly VITE_ENABLE_CONSOLE_IN_PRODUCTION?: string;
	readonly VITE_ALERT_WEBHOOK_URL?: string;
}

interface ImportMeta {
	readonly env: ImportMetaEnv;
}

export {};
