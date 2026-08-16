<!-- Compiler warnings and the `svelte-ignore` code action that suppresses them,
     plus the a11y rules whose quick fixes upstream tests in
     test/plugins/svelte/features/getCodeAction.test.ts. -->
<script>
	let href = $state('/somewhere');
</script>

<a>no href</a>

<!-- svelte-ignore a11y_missing_attribute -->
<img src="/cat.png" />

<div onclick={() => {}}>clickable div</div>

<a href={href} target="_blank">external</a>

<marquee>deprecated</marquee>
