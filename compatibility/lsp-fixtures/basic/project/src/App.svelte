<script lang="ts">
	import Counter from './Counter.svelte';
	import { emptyTodo, type Todo } from './lib';

	let todos = $state<Todo[]>([emptyTodo, { id: 2, title: 'write a gate', done: false }]);
	let name = $state('world');
</script>

<svelte:head>
	<title>{name}</title>
</svelte:head>

<main>
	<input bind:value={name} placeholder="name" />
	<Counter {todos} label={name} />

	{#await Promise.resolve(todos.length)}
		<span>counting</span>
	{:then total}
		<span>{total}</span>
	{/await}
</main>

<style>
	main {
		padding: 1rem;
	}
</style>
