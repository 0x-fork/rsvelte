<script>
	export const plain = 1;
	export const st = $state(1);
	export const raw = $state.raw([1]);
	export const der = $derived(st + 1);

	export let reassigned_state = $state(2);
	reassigned_state = 3;

	let local = 0;
</script>

<div>{plain}{st}{raw.length}{der}{reassigned_state}{local}</div>
