<script lang="ts">
	import { pendingWeight, type Todo } from './lib';

	let { todos = [] as Todo[], label = 'Counter' } = $props();

	let clicks = $state(0);
	let weight = $derived(pendingWeight(todos));

	function bump(step: number) {
		clicks += step;
	}
</script>

<section class="counter" style="color: red">
	<h1>{label}</h1>
	<button onclick={() => bump(1)} aria-label="increment">
		{clicks} / {weight}
	</button>

	{#each todos as todo (todo.id)}
		<p class:done={todo.done}>{todo.title}</p>
	{/each}

	{#if weight > 0}
		<small>still open</small>
	{:else}
		<small>all done</small>
	{/if}
</section>

<style>
	.counter {
		display: flex;
		color: #ff0000;
	}

	.done {
		text-decoration: line-through;
	}
</style>
