/** A `.svelte.ts` rune module: the entry point `compileModule` owns, and the
 * one both servers have to treat as Svelte rather than as plain TypeScript. */
export class Counter {
	count = $state(0);
	doubled = $derived(this.count * 2);

	increment(step = 1) {
		this.count += step;
	}
}

export const shared = new Counter();

export function watch(counter: Counter) {
	$effect(() => {
		console.log(counter.count);
	});
}
