<script lang="ts">
	// The losing writer's refused text, kept in memory after a stale-save
	// reload (#34; ADR 0008). Read-only and copyable: the writer copies what
	// they still want into the reloaded working copy and saves it through
	// the same revision contract. This component never merges, never
	// resubmits, and never writes the buffer anywhere.
	import type { RefusedBuffer } from '$lib/drafts/editor.svelte';

	let {
		buffers,
		onDiscard
	}: { buffers: RefusedBuffer[]; onDiscard: (index: number) => void } = $props();
</script>

<!-- One block per refused save, newest first: a later refusal is added,
     never substituted, so text the writer has not copied yet is still
     here. The newest is open by default; the reloaded working copy stays
     the page's main content. -->
{#each buffers as refused, index (index)}
	{@const narrativeCount = refused.narratives.length}
	{@const ratingCount = refused.ratings.length}
	<details class="refused" open={index === 0 && narrativeCount > 0}>
		<summary>
			{index === 0
				? 'Your unsaved text from before the reload'
				: 'Text from an earlier refused save'}
			{#if narrativeCount > 0}
				<span class="quiet-inline">
					({narrativeCount}
					{narrativeCount === 1 ? 'narrative' : 'narratives'})
				</span>
			{/if}
		</summary>
		<p class="quiet small-note">
			Another contributor saved first, so their copy is what this page now
			edits. Your save was refused and never applied. The text below is
			yours and was not saved anywhere — copy anything you still want into
			the reloaded fields, then save again through the normal revision
			check. Nothing here is merged or resubmitted for you.
		</p>
		{#if narrativeCount > 0}
			<h3>Narrative text that differed</h3>
			{#each refused.narratives as narrative (narrative.form_narrative_id)}
				<div class="narrative">
					<p class="label">
						{narrative.label}
						{#if narrative.typed_while_pending}
							<span class="pending-note">
								— typed after the refused save was sent, so it was
								never submitted
							</span>
						{/if}
					</p>
					<!-- Read-only and copyable; never an editor. -->
					<pre class="refused-text">{narrative.text}</pre>
				</div>
			{/each}
		{/if}
		{#if ratingCount > 0}
			<h3>Ratings that differed</h3>
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
										changed after the refused save was sent
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
