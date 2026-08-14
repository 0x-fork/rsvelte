import { describe, expect, it } from 'vitest';
import { nativeCompile } from '../src/plugins/native-compile.js';

describe('nativeCompile', () => {
	it('ignores compiler options whose value is undefined', async () => {
		const api = {
			options: {
				isBuild: true,
				extensions: ['.svelte'],
				emitCss: true,
				compilerOptions: {
					css: 'external',
					dev: false,
					hmr: false,
					hydratable: undefined
				}
			}
		};
		const clientFallback = { name: 'client' };
		const serverFallback = { name: 'server' };

		const [client, server] = nativeCompile(api, clientFallback, serverFallback);
		await client.configResolved();
		await server.configResolved();

		expect(api.nativeCompile).toBe(true);
		expect(client.__rsvelteNativeOptions).toEqual({
			enabled: true,
			extensions: ['.svelte'],
			emitCss: true,
			compilerOptions: {
				dev: false,
				hmr: false,
				generate: 'client'
			}
		});
		expect(server.__rsvelteNativeOptions).toEqual({
			enabled: true,
			extensions: ['.svelte'],
			emitCss: true,
			compilerOptions: {
				dev: false,
				hmr: false,
				generate: 'server'
			}
		});
	});
});
