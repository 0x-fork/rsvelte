import assert from 'node:assert/strict';

import { nativeCompile } from '../../apps/npm/vite-plugin-svelte/src/plugins/native-compile.js';

const options = {
	compilerOptions: {
		css: 'external',
		dev: false,
		hmr: false,
		runes: true
	},
	emitCss: true,
	extensions: ['.svelte'],
	isBuild: true
};
const api = { options };
const clientFallback = { name: 'client', configResolved() {} };
const serverFallback = { name: 'server' };

const plugins = nativeCompile(api, clientFallback, serverFallback);
assert.equal(plugins.length, 2);
await plugins[0].configResolved();

assert.equal(clientFallback.applyToEnvironment({ config: { consumer: 'client' } }), true);
assert.equal(clientFallback.applyToEnvironment({ config: { consumer: 'server' } }), false);
assert.equal(serverFallback.applyToEnvironment({ config: { consumer: 'client' } }), false);
assert.equal(serverFallback.applyToEnvironment({ config: { consumer: 'server' } }), true);
assert.deepEqual(clientFallback.__rsvelteNativeOptions, {
	compilerOptions: {
		dev: false,
		hmr: false,
		runes: true
	},
	emitCss: true,
	enabled: true,
	extensions: ['.svelte']
});

const dynamicApi = { options: { ...options, dynamicCompileOptions() {} } };
const dynamicClient = { name: 'client', configResolved() {} };
const dynamicPlugins = nativeCompile(dynamicApi, dynamicClient, {
	name: 'server'
});
await dynamicPlugins[0].configResolved();
assert.equal(dynamicClient.__rsvelteNativeOptions.enabled, false);

console.log('vite-plugin-svelte native Rolldown bridge: ok');
