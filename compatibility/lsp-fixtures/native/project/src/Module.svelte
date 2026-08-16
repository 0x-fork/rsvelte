<!-- A module script beside an instance script: two separate scopes, which is the
     axis test/plugins/svelte/SvelteDocument.test.ts covers. -->
<script module lang="ts">
	export const registry = new Map<string, number>();

	export function register(key: string): number {
		const next = registry.size + 1;
		registry.set(key, next);
		return next;
	}
</script>

<script lang="ts">
	let { key = 'anonymous' }: { key?: string } = $props();

	const id = register(key);
	let open = $state(false);
</script>

<details bind:open>
	<summary>{key} #{id}</summary>
	<p>{registry.size} registered</p>
</details>
