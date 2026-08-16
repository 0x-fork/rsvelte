<!-- HTML structure: nesting, siblings, void elements, comments and regions.
     The folding / symbol / selection-range surface upstream covers in
     test/plugins/html/getFoldingRange.test.ts. -->
<html>
	<head>
		<title>folding</title>
	</head>
	<body class="page">
		<!-- #region navigation -->
		<nav>
			<ul>
				<li><a href="/one">one</a></li>
				<li><a href="/two">two</a></li>
			</ul>
		</nav>
		<!-- #endregion -->

		<main
			id="content"
			data-testid="content"
		>
			<img src="/logo.png" alt="logo" />
			<br />
			<input type="text" disabled />

			<p>
				text that
				spans lines
			</p>

			<!-- a multi line
			     comment -->
		</main>
	</body>
</html>
