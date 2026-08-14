const supportedCompilerOptions = new Set([
	'accessors',
	'css',
	'customElement',
	'dev',
	'discloseVersion',
	'hmr',
	'immutable',
	'name',
	'namespace',
	'preserveComments',
	'preserveWhitespace',
	'rootDir',
	'runes'
]);

/**
 * @param {import('../types/plugin-api.d.ts').PluginAPI} api
 * @param {import('vite').Plugin & { __rsvelteNativeOptions?: object }} clientFallback
 * @param {import('vite').Plugin & { __rsvelteNativeOptions?: object }} serverFallback
 * @returns {import('vite').Plugin[]}
 */
export function nativeCompile(api, clientFallback, serverFallback) {
	configureNativeCompile(api, clientFallback, 'client');
	configureNativeCompile(api, serverFallback, 'server');
	clientFallback.applyToEnvironment = (environment) => environment.config.consumer === 'client';
	serverFallback.applyToEnvironment = (environment) => environment.config.consumer === 'server';

	return [clientFallback, serverFallback];
}

/**
 * @param {import('../types/plugin-api.d.ts').PluginAPI} api
 * @param {import('vite').Plugin & { __rsvelteNativeOptions?: object }} fallback
 * @param {'client' | 'server'} generate
 */
function configureNativeCompile(api, fallback, generate) {
	/** @type {{ enabled: boolean, extensions?: string[], emitCss?: boolean, compilerOptions?: Record<string, unknown> }} */
	const nativeOptions = { enabled: false };
	const configResolved = fallback.configResolved;

	fallback.name = 'vite-plugin-svelte:native-compile';
	fallback.__rsvelteNativeOptions = nativeOptions;
	fallback.configResolved = async function (...args) {
		if (typeof configResolved === 'function') {
			await configResolved.apply(this, args);
		}

		nativeOptions.enabled = canUseNativeCompile(api.options);
		api.nativeCompile = nativeOptions.enabled;
		if (!nativeOptions.enabled) return;

		nativeOptions.extensions = api.options.extensions;
		nativeOptions.emitCss = api.options.emitCss;
		nativeOptions.compilerOptions = Object.fromEntries(
			Object.entries(api.options.compilerOptions).filter(
				([name, value]) =>
					value !== undefined && name !== 'css' && supportedCompilerOptions.has(name)
			)
		);
		nativeOptions.compilerOptions.generate = generate;
	};
}

/**
 * @param {import('../types/options.d.ts').ResolvedOptions} options
 */
function canUseNativeCompile(options) {
	if (!options.isBuild || options.compilerOptions.hmr) return false;
	if (options.dynamicCompileOptions || options.onwarn) return false;

	return Object.entries(options.compilerOptions).every(
		([name, value]) =>
			value === undefined || (supportedCompilerOptions.has(name) && typeof value !== 'function')
	);
}
