import js from '@eslint/js';
import ts from 'typescript-eslint';
import svelte from 'eslint-plugin-svelte';
import prettier from 'eslint-config-prettier';
import globals from 'globals';

/** @type {import('eslint').Linter.Config[]} */
export default [
	js.configs.recommended,
	...ts.configs.recommended,
	...svelte.configs['flat/recommended'],
	prettier,
	...svelte.configs['flat/prettier'],
	{
		languageOptions: {
			globals: {
				...globals.browser,
				...globals.node
			}
		}
	},
	{
		files: ['**/*.svelte'],
		languageOptions: {
			parserOptions: {
				parser: ts.parser
			}
		}
	},
	{
		// Relaxed rules for auto-generated API client code
		files: ['src/lib/api/**/*.ts'],
		rules: {
			'@typescript-eslint/no-empty-object-type': 'off',
			'@typescript-eslint/no-explicit-any': 'off',
			'@typescript-eslint/ban-ts-comment': 'off'
		}
	},
	{
		ignores: ['build/', '.svelte-kit/', 'dist/', 'node_modules/']
	}
];
