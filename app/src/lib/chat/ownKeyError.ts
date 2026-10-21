// An error about the owner's own connection, in the shape the backend writes
// it (`CODE(Provider): English`, crates/ai providers/openai.rs own_key_error):
// the i18n key that words it in his language, and the provider it names.

const KEYS: Record<string, string> = {
	neboai_signed_out: 'ownKey.signedOut',
	PROVIDER_KEY_REJECTED: 'ownKey.keyRejected',
	PROVIDER_KEY_UNUSABLE: 'ownKey.keyUnusable',
};

const SHAPE = /\b([A-Za-z_]+)\(([^)]*)\):/;

/** The i18n key and provider of an own-key error in `text`; null for any other text. */
export function ownKeyError(text: string | null | undefined): { key: string; provider: string } | null {
	const m = (text ?? '').match(SHAPE);
	if (!m || !KEYS[m[1]]) return null;
	return { key: KEYS[m[1]], provider: m[2] };
}
