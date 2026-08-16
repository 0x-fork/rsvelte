<!-- Svelte 4 syntax in a Svelte 5 project: the shape both servers must still
     understand, and the one that produces migration warnings and code lenses. -->
<script>
	import { onMount } from 'svelte';

	export let title = 'legacy';
	export let items = [];

	let count = 0;

	$: doubled = count * 2;
	$: if (count > 10) {
		count = 0;
	}

	onMount(() => {
		count = 1;
	});
</script>

<h1>{title}</h1>

<button on:click={() => count++}>{count} / {doubled}</button>

{#each items as item, index (item.id)}
	<slot name="item" {item} {index} />
{:else}
	<p>empty</p>
{/each}

<slot />

<style>
	h1 {
		margin: 0;
	}
</style>
