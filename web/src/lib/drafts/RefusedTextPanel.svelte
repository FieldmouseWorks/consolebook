<script lang="ts">
	// Local text missing from the server copy, kept in memory after a
	// refused or unconfirmed save (#34; ADR 0008). Read-only and copyable:
	// this component never merges, resubmits, or persists the buffer.
	import type { RefusedBuffer } from '$lib/drafts/editor.svelte';

	let {
		buffers,
		onDiscard
	}: { buffers: RefusedBuffer[]; onDiscard: (index: number) => void } = $props();
</script>

<!-- One block per interrupted save, newest first. The newest is open by
     default; the server copy remains the page's main content. -->
{#each buffers as refused, index (index)}
	{@const narrativeCount = refused.narratives.length}
	{@const ratingCount = refused.ratings.length}
	<details class="refused" open={index === 0 && (narrativeCount > 0 || ratingCount > 0)}>
		<summary>
			{index === 0
				? 'Your local text from before the reload'
				: 'Earlier local text'}
			{#if narrativeCount > 0}
				<span class="quiet-inline">
					({narrativeCount}
					{narrativeCount === 1 ? 'narrative' : 'narratives'})
				</span>
			{/if}
		</summary>
		<p class="quiet small-note">
			{#if refused.reason === 'stale_save'}
				Another contributor saved first. Your save was refused and never
				applied. Once the latest draft loads, copy the text or repeat the
				deletion below in its fields, then save through the normal revision
				check. Nothing here is merged or resubmitted for you.
			{:else if refused.reason === 'unconfirmed_frozen'}
				The draft is now read-only, and this save could not be confirmed.
				Compare this local text with the latest
				draft when it loads. Copy anything you need elsewhere before leaving
				or discard it; it cannot be saved to this draft while read-only.
			{:else}
				The server refused this save because the draft became read-only.
				This local copy stays on this page. Compare it with the latest draft
				when it loads. Copy anything you need elsewhere before leaving or
				discard it; it cannot be saved to this draft while read-only.
			{/if}
		</p>
		{#if narrativeCount > 0}
			<h3>Narratives to recover</h3>
			{#each refused.narratives as narrative (narrative.form_narrative_id)}
				<div class="narrative">
					<p class="label">
						{narrative.label}
						{#if narrative.typed_while_pending}
							<span class="pending-note">
								— typed after the save was sent, so it was
								never submitted
							</span>
						{/if}
					</p>
					{#if narrative.text === ''}
						<p class="refused-text">
							{#if refused.reason === 'stale_save'}
								Clear this narrative. After the latest draft loads, leave its field empty and save to apply the deletion.
							{:else}
								This narrative was empty in your local copy. Compare it with the latest draft when it loads; no change can be applied while read-only.
							{/if}
						</p>
					{:else}
						<!-- Read-only and copyable; never an editor. -->
						<pre class="refused-text">{narrative.text}</pre>
					{/if}
				</div>
			{/each}
		{/if}
		{#if ratingCount > 0}
			<h3>Ratings to recover</h3>
			<table class="grid">
				<thead>
					<tr>
						<th>Competency</th>
						<th>Your value</th>
						<th>Modifiers</th>
					</tr>
				</thead>
				<tbody>
					{#each refused.ratings as rating (rating.form_competency_id)}
						<tr>
							<td>{rating.name}</td>
							<td>
								{#if rating.not_observed}
									Not observed
								{:else if rating.value !== null}
									{rating.value}
								{:else}
									<span class="quiet-inline">—</span>
								{/if}
							</td>
							<td>
								{rating.modifier_codes.join(' ')}
								{#if rating.typed_while_pending}
									<span class="pending-note">
										changed after the save was sent
									</span>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
		<div class="row route">
			<button type="button" class="secondary small" onclick={() => onDiscard(index)}>
				Discard this text
			</button>
		</div>
	</details>
{/each}

<style>
	/* Visibly the refused buffer, never the saved working copy: a distinct
	   frame and surface separate it from the fields above. */
	details.refused {
		border: 1px solid #c9a227;
		border-left: 4px solid #c9a227;
		background: light-dark(#fdf8e8, #33291a);
		border-radius: 6px;
		padding: 0.5rem 0.75rem;
		margin: 0.75rem 0;
	}
	details.refused summary {
		cursor: pointer;
		font-weight: 600;
	}
	details.refused h3 {
		margin: 0.75rem 0 0.35rem;
		font-size: 0.95rem;
	}
	.label {
		margin: 0 0 0.25rem;
		font-weight: 600;
	}
	.narrative {
		margin-bottom: 0.9rem;
	}
	/* Read-only: the browser offers selection and copy, no editing. */
	.refused-text {
		white-space: pre-wrap;
		word-break: break-word;
		font: inherit;
		margin: 0;
		padding: 0.5rem;
		border: 1px solid light-dark(#e0d6b4, #4a4030);
		border-radius: 4px;
		background: light-dark(#fffdf5, #241f16);
	}
	table.grid {
		width: 100%;
		border-collapse: collapse;
	}
	table.grid th,
	table.grid td {
		text-align: left;
		padding: 0.35rem 0.5rem;
		border-bottom: 1px solid light-dark(#e0d6b4, #4a4030);
		vertical-align: top;
	}
	.small-note {
		margin: 0.35rem 0 0.5rem;
		font-size: 0.85rem;
	}
	.row.route {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		flex-wrap: wrap;
		margin-top: 0.5rem;
	}
	.quiet-inline {
		color: #5b6672;
		font-size: 0.85rem;
	}
	.quiet {
		color: #5b6672;
	}
	.pending-note {
		font-weight: 400;
		font-size: 0.8rem;
		color: light-dark(#8a6d1f, #d8b552);
	}
	button.small {
		font-size: 0.85rem;
	}
</style>
