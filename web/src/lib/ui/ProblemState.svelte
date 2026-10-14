<script lang="ts">
	// A page that could not load explains itself in plain words and offers
	// a way back. The server's own detail stays available, but it is not the
	// headline: it names internal capabilities, not what the reader can do.
	let { status, detail }: { status: number | null; detail: string } = $props();

	const copy = $derived.by(() => {
		switch (status) {
			case 401:
				return {
					title: 'Your session has ended',
					body: 'Sign in again to continue.'
				};
			case 403:
				return {
					title: 'You do not have access to this',
					body: 'Your account is not allowed to open this page. If you need it, ask a coordinator or administrator.'
				};
			case 404:
				return {
					title: 'Not found',
					body: 'Nothing exists at this address. The link may be mistyped or out of date.'
				};
			default:
				return {
					title: 'This page could not load',
					body: 'The server could not be reached or could not answer. Try again in a moment.'
				};
		}
	});
</script>

<section class="problem" role="alert">
	<h1>{copy.title}</h1>
	<p>{copy.body}</p>
	{#if detail}
		<details>
			<summary>Details</summary>
			<p class="detail">{detail}</p>
		</details>
	{/if}
	<p class="links">
		{#if status === 401}
			<a href="/login">Sign in</a>
		{:else}
			<a href="/">Back to Home</a>
		{/if}
	</p>
</section>

<style>
	.problem {
		background: light-dark(#ffffff, #1f232c);
		border: 1px solid light-dark(#d8dce3, #303642);
		border-left: 4px solid light-dark(#b8442a, #e0795f);
		border-radius: 8px;
		padding: 1.5rem;
		max-width: 44rem;
	}
	h1 {
		margin: 0 0 0.5rem;
	}
	p {
		margin: 0 0 0.75rem;
	}
	details {
		margin: 0 0 0.75rem;
		font-size: 0.9rem;
	}
	summary {
		cursor: pointer;
		opacity: 0.8;
	}
	.detail {
		margin: 0.4rem 0 0;
		opacity: 0.8;
		overflow-wrap: anywhere;
	}
	.links {
		margin: 0;
	}
</style>
