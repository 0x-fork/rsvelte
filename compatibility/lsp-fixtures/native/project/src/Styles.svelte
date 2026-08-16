<!-- The CSS plugin surface: selectors that feed id/class completion, colours
     that feed documentColor, at-rules and nested blocks. -->
<div id="wrapper" class="card card--wide">
	<span class="title">title</span>
	<span class="subtitle">subtitle</span>
	<button id="submit" class="btn btn-primary">go</button>
</div>

<style>
	#wrapper {
		--accent: #3366ff;
		color: rgb(10, 20, 30);
		background: hsl(200, 50%, 40%);
	}

	.card {
		border: 1px solid var(--accent);
	}

	.card--wide {
		width: 100%;
	}

	.title,
	.subtitle {
		font-weight: 600;
	}

	#submit.btn-primary:hover {
		background: red;
	}

	@media (min-width: 600px) {
		.card {
			display: grid;
		}
	}
</style>
