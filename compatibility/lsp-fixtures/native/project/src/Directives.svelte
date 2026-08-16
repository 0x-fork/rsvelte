<!-- The Svelte plugin surface: every directive kind, both event syntaxes, and
     the special elements. Hover and completion on a directive name are handled
     natively on both servers. -->
<script>
	let value = $state('');
	let visible = $state(true);
	let node;

	function handle(event) {
		console.log(event.type);
	}

	function action(element) {
		return { destroy() {} };
	}
</script>

<svelte:window on:resize={handle} />
<svelte:document onvisibilitychange={handle} />

<label for="field">field</label>
<input id="field" bind:value on:input={handle} />

<div
	bind:this={node}
	class:visible
	style:color="red"
	use:action
	transition:fade
	in:fly={{ y: 8 }}
	out:fade
	animate:flip
	onclick={handle}
>
	{value}
</div>

{#if visible}
	<p>shown</p>
{:else if value}
	<p>typed</p>
{:else}
	<p>hidden</p>
{/if}

{#key value}
	<span>{value.length}</span>
{/key}

{@html '<b>raw</b>'}
{@const doubled = value.length * 2}
{@debug value}

<svelte:boundary>
	{#snippet failed(error)}
		<p>{error}</p>
	{/snippet}
	<span>{doubled}</span>
</svelte:boundary>
