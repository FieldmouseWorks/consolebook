<script lang="ts">
	import { goto, invalidateAll } from '$app/navigation';
	import { page } from '$app/state';
	import { ApiError, login } from '$lib/api';

	// Reached by another address than the one the operator published: with
	// an https:// public URL the browser drops the Secure session cookie on
	// plain HTTP, so signing in here would silently bounce (ADR 0025).
	const elsewhere = $derived.by(() => {
		const publicUrl = page.data.instance?.public_url;
		return publicUrl && publicUrl !== page.url.origin ? publicUrl : null;
	});

	let username = $state('');
	let password = $state('');
	let error = $state('');
	let busy = $state(false);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		error = '';
		busy = true;
		try {
			await login(username, password);
			await invalidateAll();
			await goto('/');
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}
</script>

<h1>Sign in</h1>
<p class="lede">Training records for your communications center.</p>

{#if elsewhere}
	<p class="notice" role="note">
		This installation is meant to be opened at <a href={elsewhere}>{elsewhere}</a>. Signing in
		from this address may not keep you signed in.
	</p>
{/if}

<form class="card" onsubmit={submit}>
	<label for="username">Username</label>
	<input id="username" autocomplete="username" required bind:value={username} />

	<label for="password">Password</label>
	<input
		id="password"
		type="password"
		autocomplete="current-password"
		required
		bind:value={password}
	/>

	{#if error}
		<p class="error" role="alert">{error}</p>
	{/if}
	<button type="submit" disabled={busy}>Sign in</button>
</form>

<p>
	Locked out with a reset code? <a href="/reset">Use a password reset code</a>.
</p>

<style>
	.notice {
		padding: 0.75rem 1rem;
		border-radius: 6px;
		border: 1px solid light-dark(#e3c37a, #6b5521);
		background: light-dark(#fff7e3, #2f2716);
	}
</style>
